//! The split stage for a whole batch of event lines at once, for processors
//! with AVX-512 (VBMI2 and VPOPCNTDQ: Ice Lake and later).
//!
//! `split_event` looks at one line through two 64-byte windows and finds its
//! separators with scalar bit operations: about 155 instructions a line. Here
//! the input is read in 64-byte blocks whatever the lines are: each block
//! gives a word of comma bits, a word of quote bits, and the positions of its
//! newlines (`vpcompressb`). Then, eight lines at a time in 64-bit lanes, each
//! line's bits are cut out of those words and its six commas located
//! (lowest set bit, counted with `vpopcntq`), its quoting checked, and its
//! `sep` word put together.
//!
//! What it accepts and what it writes are exactly `split_event`'s: the same
//! lines (three windows of 64 bytes: the event_id and its comma; from there
//! the user_id, the ts and their commas; from there three more commas and the
//! newline; sound quoting), the same `p` and `sep`, lines with an empty
//! event_id consumed and left out, and it stops at the first line it does not
//! accept, which the caller gives to the scalar path. `SS_SPLIT_CHECK=1` runs
//! both and compares.

use core::arch::x86_64::*;

use super::{ClickStage, Stage, NB};

/// Blocks looked at for one batch: more than 64 ordinary lines take.
const BLOCKS: usize = 160;

#[repr(C, align(64))]
pub struct Wide {
    /// Comma and quote bits of each block.
    commas: [u64; BLOCKS + 4],
    quotes: [u64; BLOCKS + 4],
    /// `ends[1 + i]`: where line `i`'s newline is, from the batch's start;
    /// `ends[0]` stands for the newline before the batch.
    ends: [u16; 1 + NB + 64 + 31],
}

impl Wide {
    pub fn new() -> Wide {
        let mut w = Wide { commas: [0; BLOCKS + 4], quotes: [0; BLOCKS + 4], ends: [0; 1 + NB + 64 + 31] };
        w.ends[0] = u16::MAX;
        w
    }
}

/// Whether this processor can run `split_batch`.
pub fn available() -> bool {
    is_x86_feature_detected!("avx512f")
        && is_x86_feature_detected!("avx512bw")
        && is_x86_feature_detected!("avx512vl")
        && is_x86_feature_detected!("avx512dq")
        && is_x86_feature_detected!("avx512vbmi2")
        && is_x86_feature_detected!("avx512vpopcntdq")
        && is_x86_feature_detected!("bmi2")
}

static IOTA: [u8; 64] = {
    let mut a = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        a[i] = i as u8;
        i += 1;
    }
    a
};

/// The lowest set bit of each lane: its position (64 for none), and the lane
/// without it.
#[inline]
#[target_feature(enable = "avx512f,avx512vpopcntdq")]
unsafe fn lowest(x: __m512i) -> (__m512i, __m512i) {
    let t = _mm512_and_si512(x, _mm512_sub_epi64(_mm512_setzero_si512(), x));
    (_mm512_popcnt_epi64(_mm512_sub_epi64(t, _mm512_set1_epi64(1))), _mm512_xor_si512(x, t))
}

/// Bit `at` of each lane, as 0 or 1 (0 where `at` is 64 or more, or "negative").
#[inline]
#[target_feature(enable = "avx512f")]
unsafe fn bit(x: __m512i, at: __m512i) -> __m512i {
    _mm512_and_si512(_mm512_srlv_epi64(x, at), _mm512_set1_epi64(1))
}

/// Block by block from `p0`, while blocks start below `limit`: the comma and
/// quote bits of each block into `ws`, and the positions of the newlines into
/// `ws.ends`, until there are lines enough for a batch. Returns the number of
/// newlines found.
#[inline]
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512vbmi2,popcnt")]
unsafe fn scan(ws: &mut Wide, p0: *const u8, limit: usize, ahead: usize) -> usize {
    let (comma, newline, quote) = (_mm512_set1_epi8(b',' as i8), _mm512_set1_epi8(b'\n' as i8), _mm512_set1_epi8(b'"' as i8));
    let iota = _mm512_loadu_si512(IOTA.as_ptr() as *const _);
    let ends = ws.ends.as_mut_ptr();

    // Block by block: the comma and quote bits, and the newlines' positions,
    // until there are lines enough for a batch.
    let (mut b, mut k) = (0usize, 0usize);
    while (k < NB) & (b < BLOCKS) & (64 * b < limit) {
        let q = p0.add(64 * b);
        _mm_prefetch::<_MM_HINT_T0>(q.wrapping_add(ahead) as *const i8);
        let v = _mm512_loadu_si512(q as *const _);
        let nl = _mm512_cmpeq_epi8_mask(v, newline);
        *ws.commas.get_unchecked_mut(b) = _mm512_cmpeq_epi8_mask(v, comma);
        *ws.quotes.get_unchecked_mut(b) = _mm512_cmpeq_epi8_mask(v, quote);
        let count = nl.count_ones() as usize;
        // The first eight newlines of the block (ordinary lines are longer
        // than eight bytes); a block with more is done bit by bit.
        let at = _mm_cvtepu8_epi16(_mm512_castsi512_si128(_mm512_maskz_compress_epi8(nl, iota)));
        _mm_storeu_si128(ends.add(1 + k) as *mut __m128i, _mm_add_epi16(at, _mm_set1_epi16((64 * b) as i16)));
        if count > 8 {
            let (mut m, mut j) = (nl, 0);
            while m != 0 {
                *ends.add(1 + k + j) = (64 * b + m.trailing_zeros() as usize) as u16;
                m &= m - 1;
                j += 1;
            }
        }
        k += count;
        b += 1;
    }
    k
}

/// Split lines from `data + pos` on, as many as fit a batch, while they start
/// before `fast_hi`. Returns (lines in the batch, the position after the
/// lines consumed, lines consumed, stuck). `stuck`: the line at the new
/// position is one this does not take (or none was consumed at all): the
/// caller must take it by the scalar path.
///
/// As for `split_event`, `MARGIN` bytes must be readable from every line start.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi2,avx512vpopcntdq,bmi1,bmi2,popcnt,lzcnt")]
pub unsafe fn split_batch(
    st: &mut Stage,
    ws: &mut Wide,
    data: *const u8,
    pos: usize,
    fast_hi: usize,
    ahead: usize,
) -> (usize, usize, usize, bool) {
    let p0 = data.add(pos);
    // Lines must start below this (from the batch's start).
    let limit = fast_hi - pos;
    let k = scan(ws, p0, limit, ahead);
    let ends = ws.ends.as_mut_ptr();

    let one = _mm512_set1_epi64(1);
    let ones = _mm512_set1_epi64(-1);
    let c64 = _mm512_set1_epi64(64);
    let c63 = _mm512_set1_epi64(63);
    let zero = _mm512_setzero_si512();
    let start0 = _mm512_set1_epi64(p0 as i64);
    let limit_v = _mm512_set1_epi64(limit as i64);
    let (commas, quotes) = (ws.commas.as_ptr() as *const i64, ws.quotes.as_ptr() as *const i64);
    // The bits below bit `n` of each lane (all of them for 64 or more).
    let below = |n: __m512i| _mm512_andnot_si512(_mm512_sllv_epi64(ones, n), ones);
    let after = |c: __m512i| _mm512_add_epi64(c, one);
    let before = |c: __m512i| _mm512_sub_epi64(c, one);
    let (mut n, mut i, mut stuck) = (0usize, 0usize, false);
    while (i < k) & (n + 8 <= NB) {
        // Lines i..i+8: each starts after the newline before it.
        let s = _mm512_cvtepu16_epi64(_mm_add_epi16(_mm_loadu_si128(ends.add(i) as *const __m128i), _mm_set1_epi16(1)));
        let e = _mm512_cvtepu16_epi64(_mm_loadu_si128(ends.add(i + 1) as *const __m128i));
        // The lanes that hold a line of this batch that starts in time.
        let live = _bzhi_u32(0xFF, (k - i).min(8) as u32) as u8 & _mm512_cmplt_epu64_mask(s, limit_v);
        // 64 bits of the block words from bit `at` on, out of two words. (A
        // lane that is not taken may ask for bits up to 130 past a line's
        // start: the words have room for that.)
        let cut = |words: *const i64, at: __m512i| {
            let (word, shift) = (_mm512_srli_epi64::<6>(at), _mm512_and_si512(at, c63));
            let w0 = _mm512_mask_i64gather_epi64::<8>(zero, live, word, words);
            let w1 = _mm512_mask_i64gather_epi64::<8>(zero, live, word, words.add(1));
            _mm512_or_si512(_mm512_srlv_epi64(w0, shift), _mm512_sllv_epi64(w1, _mm512_sub_epi64(c64, shift)))
        };
        let len = _mm512_sub_epi64(e, s);

        // Window 1: the event_id and its comma. (No bit from the newline on,
        // here and below: a comma that is missing is not found in the next line.)
        let in_a = below(len);
        let (c_a, q_a) = (_mm512_and_si512(cut(commas, s), in_a), _mm512_and_si512(cut(quotes, s), in_a));
        let (c0, _) = lowest(c_a);
        // Window 2, from after that comma: the user_id, the ts, their commas.
        let o1 = after(c0);
        let in_b = below(_mm512_sub_epi64(len, o1));
        let at_b = _mm512_add_epi64(s, o1);
        let (c_b, q_b) = (_mm512_and_si512(cut(commas, at_b), in_b), _mm512_and_si512(cut(quotes, at_b), in_b));
        let (c1, x) = lowest(c_b);
        let (c2, _) = lowest(x);
        // Window 3, from after the third comma: three more commas and the newline.
        let o = _mm512_add_epi64(o1, after(c2));
        let nl = _mm512_sub_epi64(len, o);
        let in_c = below(nl);
        let at_c = _mm512_add_epi64(s, o);
        let (c_c, q_c) = (_mm512_and_si512(cut(commas, at_c), in_c), _mm512_and_si512(cut(quotes, at_c), in_c));
        let (c3, y) = lowest(c_c);
        let (c4, y) = lowest(y);
        let (c5, _) = lowest(y);
        let shape = _mm512_cmplt_epu64_mask(c0, c64)
            & _mm512_cmplt_epu64_mask(c2, c64)
            & _mm512_cmpeq_epi64_mask(_mm512_popcnt_epi64(c_c), _mm512_set1_epi64(3))
            & _mm512_cmplt_epu64_mask(nl, c64);

        // Quoting: a field is quoted iff its first and last bytes are quotes,
        // and any other quote makes the line malformed. So: the same fields
        // start and end with a quote, and there are two quotes for each.
        let q_a = _mm512_and_si512(q_a, below(c0));
        let q_b = _mm512_and_si512(q_b, below(c2));
        let opens = [
            _mm512_and_si512(q_a, one),
            _mm512_and_si512(q_b, one),
            bit(q_b, after(c1)),
            _mm512_and_si512(q_c, one),
            bit(q_c, after(c3)),
            bit(q_c, after(c4)),
            bit(q_c, after(c5)),
        ];
        let closes = [
            bit(q_a, before(c0)),
            bit(q_b, before(c1)),
            bit(q_b, before(c2)),
            bit(q_c, before(c3)),
            bit(q_c, before(c4)),
            bit(q_c, before(c5)),
            bit(q_c, before(nl)),
        ];
        let mut f = opens[0];
        let mut l = closes[0];
        let mut quoted = opens[0];
        f = _mm512_or_si512(f, _mm512_slli_epi64::<1>(opens[1]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<2>(opens[2]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<3>(opens[3]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<4>(opens[4]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<5>(opens[5]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<6>(opens[6]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<1>(closes[1]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<2>(closes[2]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<3>(closes[3]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<4>(closes[4]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<5>(closes[5]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<6>(closes[6]));
        for open in &opens[1..] {
            quoted = _mm512_add_epi64(quoted, *open);
        }
        let count = _mm512_add_epi64(_mm512_add_epi64(_mm512_popcnt_epi64(q_a), _mm512_popcnt_epi64(q_b)), _mm512_popcnt_epi64(q_c));
        let sound = _mm512_cmpeq_epi64_mask(f, l) & _mm512_cmpeq_epi64_mask(count, _mm512_add_epi64(quoted, quoted));

        // Up to the first line that is not taken.
        let odd = live & !(shape & sound);
        let take = if odd != 0 { live & _bzhi_u32(0xFF, odd.trailing_zeros()) as u8 } else { live };
        // An empty event_id (nothing, or just two quotes): consumed, left out.
        let empty = _mm512_cmpeq_epi64_mask(c0, _mm512_add_epi64(opens[0], opens[0]));
        let keep = take & !empty;
        let mut sep = _mm512_or_si512(c0, _mm512_slli_epi64::<8>(_mm512_add_epi64(o1, c1)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<16>(_mm512_add_epi64(o1, c2)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<24>(_mm512_add_epi64(o, c3)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<32>(_mm512_add_epi64(o, c4)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<40>(_mm512_add_epi64(o, c5)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<48>(len));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<56>(f));
        _mm512_mask_compressstoreu_epi64(st.p.as_mut_ptr().add(n) as *mut _, keep, _mm512_add_epi64(start0, s));
        _mm512_mask_compressstoreu_epi64(st.sep.as_mut_ptr().add(n) as *mut _, keep, sep);
        n += keep.count_ones() as usize;
        i += take.count_ones() as usize;
        if odd != 0 {
            stuck = true;
            break;
        }
        if take != 0xFF {
            // Fewer than eight lines were left, or the rest start too late.
            break;
        }
    }
    (n, pos + (*ends.add(i)).wrapping_add(1) as usize, i, stuck | (i == 0))
}

/// `split_batch` for click lines: what `split_click` accepts and writes (three
/// windows of 64 bytes: the click_id and its comma; from there the user_id and
/// its comma; from there one more comma and the newline; sound quoting; a
/// line without a click_id or a user_id is consumed and left out).
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi2,avx512vpopcntdq,bmi1,bmi2,popcnt,lzcnt")]
pub unsafe fn split_clicks_batch(
    st: &mut ClickStage,
    ws: &mut Wide,
    data: *const u8,
    pos: usize,
    fast_hi: usize,
    ahead: usize,
) -> (usize, usize, usize, bool) {
    let p0 = data.add(pos);
    let limit = fast_hi - pos;
    let k = scan(ws, p0, limit, ahead);
    let ends = ws.ends.as_mut_ptr();

    let one = _mm512_set1_epi64(1);
    let ones = _mm512_set1_epi64(-1);
    let c64 = _mm512_set1_epi64(64);
    let c63 = _mm512_set1_epi64(63);
    let zero = _mm512_setzero_si512();
    let start0 = _mm512_set1_epi64(p0 as i64);
    let limit_v = _mm512_set1_epi64(limit as i64);
    let (commas, quotes) = (ws.commas.as_ptr() as *const i64, ws.quotes.as_ptr() as *const i64);
    let below = |n: __m512i| _mm512_andnot_si512(_mm512_sllv_epi64(ones, n), ones);
    let after = |c: __m512i| _mm512_add_epi64(c, one);
    let before = |c: __m512i| _mm512_sub_epi64(c, one);
    let (mut n, mut i, mut stuck) = (0usize, 0usize, false);
    while (i < k) & (n + 8 <= NB) {
        let s = _mm512_cvtepu16_epi64(_mm_add_epi16(_mm_loadu_si128(ends.add(i) as *const __m128i), _mm_set1_epi16(1)));
        let e = _mm512_cvtepu16_epi64(_mm_loadu_si128(ends.add(i + 1) as *const __m128i));
        let live = _bzhi_u32(0xFF, (k - i).min(8) as u32) as u8 & _mm512_cmplt_epu64_mask(s, limit_v);
        // 64 bits of the block words from bit `at` on (see `split_batch`).
        let cut = |words: *const i64, at: __m512i| {
            let (word, shift) = (_mm512_srli_epi64::<6>(at), _mm512_and_si512(at, c63));
            let w0 = _mm512_mask_i64gather_epi64::<8>(zero, live, word, words);
            let w1 = _mm512_mask_i64gather_epi64::<8>(zero, live, word, words.add(1));
            _mm512_or_si512(_mm512_srlv_epi64(w0, shift), _mm512_sllv_epi64(w1, _mm512_sub_epi64(c64, shift)))
        };
        let len = _mm512_sub_epi64(e, s);

        // Window 1: the click_id and its comma (no bit from the newline on).
        let in_a = below(len);
        let (c_a, q_a) = (_mm512_and_si512(cut(commas, s), in_a), _mm512_and_si512(cut(quotes, s), in_a));
        let (c0, _) = lowest(c_a);
        // Window 2, from after that comma: the user_id and its comma.
        let o1 = after(c0);
        let in_b = below(_mm512_sub_epi64(len, o1));
        let at_b = _mm512_add_epi64(s, o1);
        let (c_b, q_b) = (_mm512_and_si512(cut(commas, at_b), in_b), _mm512_and_si512(cut(quotes, at_b), in_b));
        let (c1, _) = lowest(c_b);
        // Window 3, from after the second comma: one more comma and the newline.
        let o = _mm512_add_epi64(o1, after(c1));
        let nl = _mm512_sub_epi64(len, o);
        let in_c = below(nl);
        let at_c = _mm512_add_epi64(s, o);
        let (c_c, q_c) = (_mm512_and_si512(cut(commas, at_c), in_c), _mm512_and_si512(cut(quotes, at_c), in_c));
        let (c2, _) = lowest(c_c);
        let shape = _mm512_cmplt_epu64_mask(c0, c64)
            & _mm512_cmplt_epu64_mask(c1, c64)
            & _mm512_cmpeq_epi64_mask(_mm512_popcnt_epi64(c_c), one)
            & _mm512_cmplt_epu64_mask(nl, c64);

        let q_a = _mm512_and_si512(q_a, below(c0));
        let q_b = _mm512_and_si512(q_b, below(c1));
        let opens = [_mm512_and_si512(q_a, one), _mm512_and_si512(q_b, one), _mm512_and_si512(q_c, one), bit(q_c, after(c2))];
        let closes = [bit(q_a, before(c0)), bit(q_b, before(c1)), bit(q_c, before(c2)), bit(q_c, before(nl))];
        let mut f = opens[0];
        f = _mm512_or_si512(f, _mm512_slli_epi64::<1>(opens[1]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<2>(opens[2]));
        f = _mm512_or_si512(f, _mm512_slli_epi64::<3>(opens[3]));
        let mut l = closes[0];
        l = _mm512_or_si512(l, _mm512_slli_epi64::<1>(closes[1]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<2>(closes[2]));
        l = _mm512_or_si512(l, _mm512_slli_epi64::<3>(closes[3]));
        let quoted = _mm512_add_epi64(_mm512_add_epi64(opens[0], opens[1]), _mm512_add_epi64(opens[2], opens[3]));
        let count = _mm512_add_epi64(_mm512_add_epi64(_mm512_popcnt_epi64(q_a), _mm512_popcnt_epi64(q_b)), _mm512_popcnt_epi64(q_c));
        let sound = _mm512_cmpeq_epi64_mask(f, l) & _mm512_cmpeq_epi64_mask(count, _mm512_add_epi64(quoted, quoted));

        let odd = live & !(shape & sound);
        let take = if odd != 0 { live & _bzhi_u32(0xFF, odd.trailing_zeros()) as u8 } else { live };
        // No click_id, or no user_id (nothing, or just two quotes): left out.
        let empty = _mm512_cmpeq_epi64_mask(c0, _mm512_add_epi64(opens[0], opens[0]))
            | _mm512_cmpeq_epi64_mask(c1, _mm512_add_epi64(opens[1], opens[1]));
        let keep = take & !empty;
        let mut sep = _mm512_or_si512(c0, _mm512_slli_epi64::<8>(_mm512_add_epi64(o1, c1)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<16>(_mm512_add_epi64(o, c2)));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<24>(len));
        sep = _mm512_or_si512(sep, _mm512_slli_epi64::<56>(f));
        _mm512_mask_compressstoreu_epi64(st.p.as_mut_ptr().add(n) as *mut _, keep, _mm512_add_epi64(start0, s));
        _mm512_mask_compressstoreu_epi64(st.sep.as_mut_ptr().add(n) as *mut _, keep, sep);
        n += keep.count_ones() as usize;
        i += take.count_ones() as usize;
        if odd != 0 {
            stuck = true;
            break;
        }
        if take != 0xFF {
            break;
        }
    }
    (n, pos + (*ends.add(i)).wrapping_add(1) as usize, i, stuck | (i == 0))
}
