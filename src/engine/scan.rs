//! Byte scanning layer: locate the next "interesting" byte fast.
//!
//! Two searches power the engine:
//!
//! - [`find_wake`]: the next byte that may start a boundary or structural construct (see
//!   [`is_wake_byte`]).
//! - [`find_ascii`]: the first ASCII byte at or after a position, i.e. the end of a multibyte run.
//!
//! Both use a hybrid scheme: the first words are checked with an inline SWAR
//! test (no call overhead), and longer clean stretches are handed off to a
//! SIMD continuation selected once per run by [`Scan`] (AVX2 / SSE2 on
//! x86_64, runtime-detected; SWAR loops elsewhere).
//!
//! Why inline SWAR first: on Windows x64, ymm6-15 / xmm6-15 are callee-saved,
//! so a SIMD scanner function pays a 4-5 register spill/restore prologue per
//! call. In mixed text, events are only ~10 bytes apart, which makes the
//! prologue cost exceed the SIMD benefit; the inline words cover exactly that
//! range. The SIMD scanners only take over once a long clean stretch is
//! confirmed, amortizing their prologue at 16-32 bytes per step.
// Hot-path helpers are intentionally force-inlined.
#![allow(clippy::inline_always)]

use crate::classify::is_wake_byte;

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
const fn wake_in_word(w: u64) -> bool {
    (w & SWAR_HI != 0)
        || has_byte(w, b'`')
        || has_byte(w, b'[')
        || has_byte(w, b']')
        || has_byte(w, b'<')
        || has_byte(w, b'*')
        || has_byte(w, b'~')
}

// Long-range scanner continuations: SWAR loops used directly on non-x86_64
// and as the fallback behind the SIMD scanners on x86_64 (see [`Scan`]).
// On x86_64 the SSE2 scanners are always available, so the SWAR loops are
// only referenced on other architectures.
#[cfg_attr(target_arch = "x86_64", allow(dead_code))]
mod scalar {
    use super::{SWAR_HI, is_wake_byte, wake_in_word};

    /// Find the next wake byte at or after `from`; returns `input.len()` if none.
    pub fn find_wake(input: &[u8], from: usize) -> usize {
        let len = input.len();
        let mut i = from;
        while i + 8 <= len {
            // SAFETY: bounds checked by the loop condition.
            let w =
                u64::from_le_bytes(unsafe { input.get_unchecked(i..i + 8) }.try_into().unwrap());
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

    /// Find the first ASCII byte (`< 0x80`) at or after `from`; `input.len()` if none.
    pub fn find_ascii(input: &[u8], from: usize) -> usize {
        let len = input.len();
        let mut i = from;
        while i + 8 <= len {
            // SAFETY: bounds checked by the loop condition.
            let w =
                u64::from_le_bytes(unsafe { input.get_unchecked(i..i + 8) }.try_into().unwrap());
            if w & SWAR_HI != SWAR_HI {
                // SAFETY: the word contains an ASCII byte within `[i, i + 8)`.
                while unsafe { *input.get_unchecked(i) } >= 0x80 {
                    i += 1;
                }
                return i;
            }
            i += 8;
        }
        while i < len && input[i] >= 0x80 {
            i += 1;
        }
        i
    }
}

/// Find the next wake byte at or after `from`; returns `input.len()` if none.
///
/// The first words are checked inline: in mixed text the next wake byte is
/// usually only a few bytes away, and a function call into the SIMD scanner
/// would cost more than it saves (call + spill restore on Windows). Longer
/// clean stretches are handed off to the SIMD scanner.
#[inline]
pub(super) fn find_wake(input: &[u8], from: usize, simd: fn(&[u8], usize) -> usize) -> usize {
    let len = input.len();
    if from + 16 <= len {
        // SAFETY: `from + 16 <= len`.
        let w0 = u64::from_le_bytes(
            unsafe { input.get_unchecked(from..from + 8) }
                .try_into()
                .unwrap(),
        );
        if wake_in_word(w0) {
            return wake_in_first_word(input, from);
        }
        let w1 = u64::from_le_bytes(
            unsafe { input.get_unchecked(from + 8..from + 16) }
                .try_into()
                .unwrap(),
        );
        if wake_in_word(w1) {
            return wake_in_first_word(input, from + 8);
        }
        return simd(input, from + 16);
    }
    if from + 8 <= len {
        // SAFETY: `from + 8 <= len`.
        let w = u64::from_le_bytes(
            unsafe { input.get_unchecked(from..from + 8) }
                .try_into()
                .unwrap(),
        );
        if wake_in_word(w) {
            return wake_in_first_word(input, from);
        }
        return simd(input, from + 8);
    }
    let mut i = from;
    while i < len {
        if is_wake_byte(input[i]) {
            return i;
        }
        i += 1;
    }
    len
}

/// Locate the wake byte inside a word known to contain one.
#[inline(always)]
fn wake_in_first_word(input: &[u8], from: usize) -> usize {
    let mut k = from;
    // SAFETY: the 8 bytes at `from` contain a wake byte.
    while !is_wake_byte(unsafe { *input.get_unchecked(k) }) {
        k += 1;
    }
    k
}

/// Find the first ASCII byte (`< 0x80`) at or after `from`; `input.len()` if none.
///
/// Same hybrid scheme as [`find_wake`]: inline first words, SIMD for long
/// multibyte runs.
#[inline]
pub(super) fn find_ascii(input: &[u8], from: usize, simd: fn(&[u8], usize) -> usize) -> usize {
    let len = input.len();
    if from + 16 <= len {
        // SAFETY: `from + 16 <= len`.
        let w0 = u64::from_le_bytes(
            unsafe { input.get_unchecked(from..from + 8) }
                .try_into()
                .unwrap(),
        );
        if w0 & SWAR_HI != SWAR_HI {
            return ascii_in_first_word(input, from);
        }
        let w1 = u64::from_le_bytes(
            unsafe { input.get_unchecked(from + 8..from + 16) }
                .try_into()
                .unwrap(),
        );
        if w1 & SWAR_HI != SWAR_HI {
            return ascii_in_first_word(input, from + 8);
        }
        // Both words are non-ASCII: hand off to the long-range scanner.
        return simd(input, from);
    }
    if from + 8 <= len {
        // SAFETY: `from + 8 <= len`.
        let w = u64::from_le_bytes(
            unsafe { input.get_unchecked(from..from + 8) }
                .try_into()
                .unwrap(),
        );
        if w & SWAR_HI != SWAR_HI {
            return ascii_in_first_word(input, from);
        }
        return simd(input, from);
    }
    let mut i = from;
    while i < len && input[i] >= 0x80 {
        i += 1;
    }
    i
}

/// Locate the first ASCII byte inside a word known to contain one.
#[inline(always)]
fn ascii_in_first_word(input: &[u8], from: usize) -> usize {
    let mut i = from;
    // SAFETY: the 8 bytes at `from` contain an ASCII byte.
    while unsafe { *input.get_unchecked(i) } >= 0x80 {
        i += 1;
    }
    i
}

/// SIMD scanners for x86_64: AVX2 (32 bytes/step) when available, SSE2
/// (16 bytes/step, part of the x86_64 baseline) otherwise.
#[cfg(target_arch = "x86_64")]
mod x86 {
    // The `i32 as u32` casts below are two's-complement bit reinterpretations
    // of movemask results.
    #![allow(clippy::cast_sign_loss)]

    use core::arch::x86_64::{
        __m128i, __m256i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128,
        _mm256_cmpeq_epi8, _mm256_loadu_si256, _mm256_movemask_epi8, _mm256_or_si256,
    };

    use super::is_wake_byte;

    // The compare constants live in .rodata and are referenced through memory
    // operands. Keeping the loops within the volatile register set avoids the
    // callee-saved XMM prologue, which would otherwise dominate short scans
    // (ymm6-15 are non-volatile on Windows x64).
    #[rustfmt::skip]
    static WAKE_BYTES: [[u8; 32]; 6] = [
        [b'`'; 32], [b'['; 32], [b']'; 32], [b'<'; 32], [b'*'; 32], [b'~'; 32],
    ];
    #[rustfmt::skip]
    static WAKE_BYTES_128: [[u8; 16]; 6] = [
        [b'`'; 16], [b'['; 16], [b']'; 16], [b'<' ; 16], [b'*'; 16], [b'~'; 16],
    ];

    /// Find the next wake byte at or after `from`; returns `input.len()` if none.
    ///
    /// # Safety
    ///
    /// Requires AVX2 support at runtime.
    #[target_feature(enable = "avx2")]
    pub unsafe fn find_wake_avx2(input: &[u8], from: usize) -> usize {
        let len = input.len();
        let mut i = from;
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len`, so the 32-byte read is in bounds.
            let c = unsafe { input.as_ptr().add(i).cast::<__m256i>().read_unaligned() };
            // The `avx2` target feature is enabled for this function, so the
            // intrinsics below need no `unsafe` block.
            let mut m = _mm256_cmpeq_epi8(c, loadu(&WAKE_BYTES[0]));
            m = _mm256_or_si256(m, _mm256_cmpeq_epi8(c, loadu(&WAKE_BYTES[1])));
            m = _mm256_or_si256(m, _mm256_cmpeq_epi8(c, loadu(&WAKE_BYTES[2])));
            m = _mm256_or_si256(m, _mm256_cmpeq_epi8(c, loadu(&WAKE_BYTES[3])));
            m = _mm256_or_si256(m, _mm256_cmpeq_epi8(c, loadu(&WAKE_BYTES[4])));
            m = _mm256_or_si256(m, _mm256_cmpeq_epi8(c, loadu(&WAKE_BYTES[5])));
            // `movemask(c)` already carries the sign bit of every byte, which
            // marks the `>= 0x80` bytes; merge it in the GPR domain.
            let mask = _mm256_movemask_epi8(m) | _mm256_movemask_epi8(c);
            if mask != 0 {
                return i + (mask as u32).trailing_zeros() as usize;
            }
            i += 32;
        }
        while i < len && !is_wake_byte(input[i]) {
            i += 1;
        }
        i
    }

    /// Load a 32-byte constant as an AVX2 vector (memory operand).
    ///
    /// # Safety
    ///
    /// Requires AVX2 support at runtime.
    #[target_feature(enable = "avx2")]
    fn loadu(k: &[u8; 32]) -> __m256i {
        // SAFETY: AVX2 is enabled for the caller; a 32-byte read from a
        // 32-byte object is in bounds and alignment does not matter (loadu).
        unsafe { _mm256_loadu_si256(k.as_ptr().cast()) }
    }

    /// Find the first ASCII byte (`< 0x80`) at or after `from`; `input.len()` if none.
    ///
    /// # Safety
    ///
    /// Requires AVX2 support at runtime.
    #[target_feature(enable = "avx2")]
    pub unsafe fn find_ascii_avx2(input: &[u8], from: usize) -> usize {
        let len = input.len();
        let mut i = from;
        while i + 32 <= len {
            // SAFETY: `i + 32 <= len`, so the 32-byte read is in bounds.
            let c = unsafe { input.as_ptr().add(i).cast::<__m256i>().read_unaligned() };
            // The mask holds one sign bit per byte, so all-ones means "no
            // ASCII byte in the chunk".
            let m = _mm256_movemask_epi8(c);
            if m != -1 {
                return i + ((!m) as u32).trailing_zeros() as usize;
            }
            i += 32;
        }
        while i < len && input[i] >= 0x80 {
            i += 1;
        }
        i
    }

    /// Find the next wake byte at or after `from`; returns `input.len()` if none.
    ///
    /// # Safety
    ///
    /// SSE2 is part of the x86_64 baseline, so this is always safe to call.
    #[target_feature(enable = "sse2")]
    pub unsafe fn find_wake_sse2(input: &[u8], from: usize) -> usize {
        let len = input.len();
        let mut i = from;
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len`, so the 16-byte read is in bounds.
            let c = unsafe { input.as_ptr().add(i).cast::<__m128i>().read_unaligned() };
            // SSE2 is the x86_64 baseline; these intrinsics are safe calls.
            let mut m = _mm_cmpeq_epi8(c, loadu128(&WAKE_BYTES_128[0]));
            m = _mm_or_si128(m, _mm_cmpeq_epi8(c, loadu128(&WAKE_BYTES_128[1])));
            m = _mm_or_si128(m, _mm_cmpeq_epi8(c, loadu128(&WAKE_BYTES_128[2])));
            m = _mm_or_si128(m, _mm_cmpeq_epi8(c, loadu128(&WAKE_BYTES_128[3])));
            m = _mm_or_si128(m, _mm_cmpeq_epi8(c, loadu128(&WAKE_BYTES_128[4])));
            m = _mm_or_si128(m, _mm_cmpeq_epi8(c, loadu128(&WAKE_BYTES_128[5])));
            let mask = _mm_movemask_epi8(m) | _mm_movemask_epi8(c);
            if mask != 0 {
                return i + (mask as u32).trailing_zeros() as usize;
            }
            i += 16;
        }
        while i < len && !is_wake_byte(input[i]) {
            i += 1;
        }
        i
    }

    /// Load a 16-byte constant as an SSE2 vector (memory operand).
    ///
    /// # Safety
    ///
    /// The caller must have SSE2 enabled (always true on x86_64).
    #[target_feature(enable = "sse2")]
    fn loadu128(k: &[u8; 16]) -> __m128i {
        // SAFETY: SSE2 is enabled for the caller; a 16-byte read from a
        // 16-byte object is in bounds and alignment does not matter (loadu).
        unsafe { _mm_loadu_si128(k.as_ptr().cast()) }
    }

    /// Find the first ASCII byte (`< 0x80`) at or after `from`; `input.len()` if none.
    ///
    /// # Safety
    ///
    /// SSE2 is part of the x86_64 baseline, so this is always safe to call.
    #[target_feature(enable = "sse2")]
    pub unsafe fn find_ascii_sse2(input: &[u8], from: usize) -> usize {
        let len = input.len();
        let mut i = from;
        while i + 16 <= len {
            // SAFETY: `i + 16 <= len`, so the 16-byte read is in bounds.
            let c = unsafe { input.as_ptr().add(i).cast::<__m128i>().read_unaligned() };
            // SSE2 is the x86_64 baseline, so `_mm_movemask_epi8` is a safe call.
            let m = _mm_movemask_epi8(c);
            if m != 0xffff {
                return i + (!(m as u32)).trailing_zeros() as usize;
            }
            i += 16;
        }
        while i < len && input[i] >= 0x80 {
            i += 1;
        }
        i
    }
}

/// The scanner pair used by the [`Formatter`](super::Formatter), selected
/// once per run.
#[derive(Clone, Copy)]
pub(super) struct Scan {
    /// Find the next byte that needs the scanner's attention.
    wake: fn(&[u8], usize) -> usize,
    /// Find the next ASCII byte (`< 0x80`).
    ascii: fn(&[u8], usize) -> usize,
}

impl Scan {
    /// Pick the best implementation for the current CPU.
    #[must_use]
    pub(super) fn select() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            use std::sync::OnceLock;
            static SCAN: OnceLock<Scan> = OnceLock::new();
            *SCAN.get_or_init(|| {
                if std::arch::is_x86_feature_detected!("avx2") {
                    // SAFETY: AVX2 support was just detected at runtime.
                    Self {
                        wake: |i, f| unsafe { x86::find_wake_avx2(i, f) },
                        ascii: |i, f| unsafe { x86::find_ascii_avx2(i, f) },
                    }
                } else {
                    // SAFETY: SSE2 is part of the x86_64 baseline.
                    Self {
                        wake: |i, f| unsafe { x86::find_wake_sse2(i, f) },
                        ascii: |i, f| unsafe { x86::find_ascii_sse2(i, f) },
                    }
                }
            })
        }
        #[cfg(not(target_arch = "x86_64"))]
        Self {
            wake: scalar::find_wake,
            ascii: scalar::find_ascii,
        }
    }

    /// The long-range continuation for [`find_wake`].
    pub(super) fn wake(&self) -> fn(&[u8], usize) -> usize {
        self.wake
    }

    /// The long-range continuation for [`find_ascii`].
    pub(super) fn ascii(&self) -> fn(&[u8], usize) -> usize {
        self.ascii
    }
}
