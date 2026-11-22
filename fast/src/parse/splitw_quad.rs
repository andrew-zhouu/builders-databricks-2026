// The batch splitters with FOUR lines at a time in 256-bit registers, for
// parser instance 3 (see parse/wide_quad.rs): `split_batch` and
// `split_clicks_batch` of parse/splitw.rs lane for lane (derived from them),
// with what `split_batch5` uses in place of VBMI2 and VPOPCNTDQ. EVEX
// encodings of 256-bit instructions, no 512-bit register
// (scripts/remote/quadcheck.sh). This file is included by parse/splitw.rs as
// its module `quad`.

/// `lowest5` on four lanes.
#[inline]
#[target_feature(enable = "avx512f,avx512vl,avx512cd")]
unsafe fn lowest4(x: __m256i) -> (__m256i, __m256i) {
    let t = _mm256_and_si256(x, _mm256_sub_epi64(_mm256_setzero_si256(), x));
    let under = _mm256_sub_epi64(t, _mm256_set1_epi64x(1));
    (_mm256_sub_epi64(_mm256_set1_epi64x(64), _mm256_lzcnt_epi64(under)), _mm256_xor_si256(x, t))
}

/// `ones5` on four lanes.
#[inline(always)]
unsafe fn ones4(x: __m256i) -> __m256i {
    let half = _mm256_set1_epi8(0x0F);
    let table = _mm256_broadcastsi128_si256(_mm_setr_epi8(0, 1, 1, 2, 1, 2, 2, 3, 1, 2, 2, 3, 2, 3, 3, 4));
    let lo = _mm256_shuffle_epi8(table, _mm256_and_si256(x, half));
    let hi = _mm256_shuffle_epi8(table, _mm256_and_si256(_mm256_srli_epi16::<4>(x), half));
    _mm256_sad_epu8(_mm256_add_epi8(lo, hi), _mm256_setzero_si256())
}

/// Bit `at` of each lane, as 0 or 1 (0 where `at` is 64 or more, or "negative").
#[inline(always)]
unsafe fn bit4(x: __m256i, at: __m256i) -> __m256i {
    _mm256_and_si256(_mm256_srlv_epi64(x, at), _mm256_set1_epi64x(1))
}

/// `scan5` with 256-bit compares, two to a block. An ordinary function, and
/// never part of another: with AVX-512 enabled the compiler makes one 512-bit
/// compare of the two.
#[inline(never)]
unsafe fn scan4(ws: &mut Wide, p0: *const u8, limit: usize, ahead: usize) -> usize {
    let (comma, newline, quote) = (_mm256_set1_epi8(b',' as i8), _mm256_set1_epi8(b'\n' as i8), _mm256_set1_epi8(b'"' as i8));
    let ends = ws.ends.as_mut_ptr();
    let (mut b, mut k) = (0usize, 0usize);
    while (k < NB) & (b < BLOCKS) & (64 * b < limit) {
        let q = p0.add(64 * b);
        _mm_prefetch::<_MM_HINT_T0>(q.wrapping_add(ahead) as *const i8);
        let v0 = _mm256_loadu_si256(q as *const __m256i);
        let v1 = _mm256_loadu_si256(q.add(32) as *const __m256i);
        let bits = |c: __m256i| {
            (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v0, c)) as u32 as u64)
                | (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v1, c)) as u32 as u64) << 32
        };
        let nl = bits(newline);
        *ws.commas.get_unchecked_mut(b) = bits(comma);
        *ws.quotes.get_unchecked_mut(b) = bits(quote);
        let count = nl.count_ones() as usize;
        let mut m = nl;
        for j in 0..4 {
            *ends.add(1 + k + j) = (64 * b + m.trailing_zeros() as usize) as u16;
            m &= m.wrapping_sub(1);
        }
        if count > 4 {
            let mut j = 4;
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

/// `split_batch`, four lines at a time (see there).
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512cd,bmi1,bmi2,popcnt,lzcnt")]
pub unsafe fn split_batch4(
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
    let k = scan4(ws, p0, limit, ahead);
    let ends = ws.ends.as_mut_ptr();

    let one = _mm256_set1_epi64x(1);
    let ones = _mm256_set1_epi64x(-1);
    let c64 = _mm256_set1_epi64x(64);
    let c63 = _mm256_set1_epi64x(63);
    let zero = _mm256_setzero_si256();
    let start0 = _mm256_set1_epi64x(p0 as i64);
    let limit_v = _mm256_set1_epi64x(limit as i64);
    let (commas, quotes) = (ws.commas.as_ptr() as *const i64, ws.quotes.as_ptr() as *const i64);
    // The bits below bit `n` of each lane (all of them for 64 or more).
    let below = |n: __m256i| _mm256_andnot_si256(_mm256_sllv_epi64(ones, n), ones);
    let after = |c: __m256i| _mm256_add_epi64(c, one);
    let before = |c: __m256i| _mm256_sub_epi64(c, one);
    let (mut n, mut i, mut stuck) = (0usize, 0usize, false);
    while (i < k) & (n + 4 <= NB) {
        // Lines i..i+8: each starts after the newline before it.
        let s = _mm256_cvtepu16_epi64(_mm_add_epi16(_mm_loadu_si128(ends.add(i) as *const __m128i), _mm_set1_epi16(1)));
        let e = _mm256_cvtepu16_epi64(_mm_loadu_si128(ends.add(i + 1) as *const __m128i));
        // The lanes that hold a line of this batch that starts in time.
        let live = _bzhi_u32(0x0F, (k - i).min(4) as u32) as u8 & _mm256_cmplt_epu64_mask(s, limit_v);
        // 64 bits of the block words from bit `at` on, out of two words. (A
        // lane that is not taken may ask for bits up to 130 past a line's
        // start: the words have room for that.)
        let cut = |words: *const i64, at: __m256i| {
            let (word, shift) = (_mm256_srli_epi64::<6>(at), _mm256_and_si256(at, c63));
            let w0 = _mm256_mmask_i64gather_epi64::<8>(zero, live, word, words);
            let w1 = _mm256_mmask_i64gather_epi64::<8>(zero, live, word, words.add(1));
            _mm256_or_si256(_mm256_srlv_epi64(w0, shift), _mm256_sllv_epi64(w1, _mm256_sub_epi64(c64, shift)))
        };
        let len = _mm256_sub_epi64(e, s);

        // Window 1: the event_id and its comma. (No bit from the newline on,
        // here and below: a comma that is missing is not found in the next line.)
        let in_a = below(len);
        let (c_a, q_a) = (_mm256_and_si256(cut(commas, s), in_a), _mm256_and_si256(cut(quotes, s), in_a));
        let (c0, _) = lowest4(c_a);
        // Window 2, from after that comma: the user_id, the ts, their commas.
        let o1 = after(c0);
        let in_b = below(_mm256_sub_epi64(len, o1));
        let at_b = _mm256_add_epi64(s, o1);
        let (c_b, q_b) = (_mm256_and_si256(cut(commas, at_b), in_b), _mm256_and_si256(cut(quotes, at_b), in_b));
        let (c1, x) = lowest4(c_b);
        let (c2, _) = lowest4(x);
        // Window 3, from after the third comma: three more commas and the newline.
        let o = _mm256_add_epi64(o1, after(c2));
        let nl = _mm256_sub_epi64(len, o);
        let in_c = below(nl);
        let at_c = _mm256_add_epi64(s, o);
        let (c_c, q_c) = (_mm256_and_si256(cut(commas, at_c), in_c), _mm256_and_si256(cut(quotes, at_c), in_c));
        let (c3, y) = lowest4(c_c);
        let (c4, y) = lowest4(y);
        let (c5, _) = lowest4(y);
        let shape = _mm256_cmplt_epu64_mask(c0, c64)
            & _mm256_cmplt_epu64_mask(c2, c64)
            & _mm256_cmpeq_epi64_mask(ones4(c_c), _mm256_set1_epi64x(3))
            & _mm256_cmplt_epu64_mask(nl, c64);

        // Quoting: a field is quoted iff its first and last bytes are quotes,
        // and any other quote makes the line malformed. So: the same fields
        // start and end with a quote, and there are two quotes for each.
        let q_a = _mm256_and_si256(q_a, below(c0));
        let q_b = _mm256_and_si256(q_b, below(c2));
        let opens = [
            _mm256_and_si256(q_a, one),
            _mm256_and_si256(q_b, one),
            bit4(q_b, after(c1)),
            _mm256_and_si256(q_c, one),
            bit4(q_c, after(c3)),
            bit4(q_c, after(c4)),
            bit4(q_c, after(c5)),
        ];
        let closes = [
            bit4(q_a, before(c0)),
            bit4(q_b, before(c1)),
            bit4(q_b, before(c2)),
            bit4(q_c, before(c3)),
            bit4(q_c, before(c4)),
            bit4(q_c, before(c5)),
            bit4(q_c, before(nl)),
        ];
        let mut f = opens[0];
        let mut l = closes[0];
        let mut quoted = opens[0];
        f = _mm256_or_si256(f, _mm256_slli_epi64::<1>(opens[1]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<2>(opens[2]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<3>(opens[3]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<4>(opens[4]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<5>(opens[5]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<6>(opens[6]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<1>(closes[1]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<2>(closes[2]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<3>(closes[3]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<4>(closes[4]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<5>(closes[5]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<6>(closes[6]));
        for open in &opens[1..] {
            quoted = _mm256_add_epi64(quoted, *open);
        }
        let count = _mm256_add_epi64(_mm256_add_epi64(ones4(q_a), ones4(q_b)), ones4(q_c));
        let sound = _mm256_cmpeq_epi64_mask(f, l) & _mm256_cmpeq_epi64_mask(count, _mm256_add_epi64(quoted, quoted));

        // Up to the first line that is not taken.
        let odd = live & !(shape & sound);
        let take = if odd != 0 { live & _bzhi_u32(0x0F, odd.trailing_zeros()) as u8 } else { live };
        // An empty event_id (nothing, or just two quotes): consumed, left out.
        let empty = _mm256_cmpeq_epi64_mask(c0, _mm256_add_epi64(opens[0], opens[0]));
        let keep = take & !empty;
        let mut sep = _mm256_or_si256(c0, _mm256_slli_epi64::<8>(_mm256_add_epi64(o1, c1)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<16>(_mm256_add_epi64(o1, c2)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<24>(_mm256_add_epi64(o, c3)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<32>(_mm256_add_epi64(o, c4)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<40>(_mm256_add_epi64(o, c5)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<48>(len));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<56>(f));
        // (Compressed in a register and stored whole, eight lanes: there is
        // room for them, `n + 8 <= NB`. The compressing store to memory is
        // the same thing on Intel and many times slower on AMD's Zen 4.)
        _mm256_storeu_si256(st.p.as_mut_ptr().add(n) as *mut _, _mm256_maskz_compress_epi64(keep, _mm256_add_epi64(start0, s)));
        _mm256_storeu_si256(st.sep.as_mut_ptr().add(n) as *mut _, _mm256_maskz_compress_epi64(keep, sep));
        n += keep.count_ones() as usize;
        i += take.count_ones() as usize;
        if odd != 0 {
            stuck = true;
            break;
        }
        if take != 0x0F {
            // Fewer than eight lines were left, or the rest start too late.
            break;
        }
    }
    (n, pos + (*ends.add(i)).wrapping_add(1) as usize, i, stuck | (i == 0))
}

#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512cd,bmi1,bmi2,popcnt,lzcnt")]
pub unsafe fn split_clicks_batch4(
    st: &mut ClickStage,
    ws: &mut Wide,
    data: *const u8,
    pos: usize,
    fast_hi: usize,
    ahead: usize,
) -> (usize, usize, usize, bool) {
    let p0 = data.add(pos);
    let limit = fast_hi - pos;
    let k = scan4(ws, p0, limit, ahead);
    let ends = ws.ends.as_mut_ptr();

    let one = _mm256_set1_epi64x(1);
    let ones = _mm256_set1_epi64x(-1);
    let c64 = _mm256_set1_epi64x(64);
    let c63 = _mm256_set1_epi64x(63);
    let zero = _mm256_setzero_si256();
    let start0 = _mm256_set1_epi64x(p0 as i64);
    let limit_v = _mm256_set1_epi64x(limit as i64);
    let (commas, quotes) = (ws.commas.as_ptr() as *const i64, ws.quotes.as_ptr() as *const i64);
    let below = |n: __m256i| _mm256_andnot_si256(_mm256_sllv_epi64(ones, n), ones);
    let after = |c: __m256i| _mm256_add_epi64(c, one);
    let before = |c: __m256i| _mm256_sub_epi64(c, one);
    let (mut n, mut i, mut stuck) = (0usize, 0usize, false);
    while (i < k) & (n + 4 <= NB) {
        let s = _mm256_cvtepu16_epi64(_mm_add_epi16(_mm_loadu_si128(ends.add(i) as *const __m128i), _mm_set1_epi16(1)));
        let e = _mm256_cvtepu16_epi64(_mm_loadu_si128(ends.add(i + 1) as *const __m128i));
        let live = _bzhi_u32(0x0F, (k - i).min(4) as u32) as u8 & _mm256_cmplt_epu64_mask(s, limit_v);
        // 64 bits of the block words from bit `at` on (see `split_batch`).
        let cut = |words: *const i64, at: __m256i| {
            let (word, shift) = (_mm256_srli_epi64::<6>(at), _mm256_and_si256(at, c63));
            let w0 = _mm256_mmask_i64gather_epi64::<8>(zero, live, word, words);
            let w1 = _mm256_mmask_i64gather_epi64::<8>(zero, live, word, words.add(1));
            _mm256_or_si256(_mm256_srlv_epi64(w0, shift), _mm256_sllv_epi64(w1, _mm256_sub_epi64(c64, shift)))
        };
        let len = _mm256_sub_epi64(e, s);

        // Window 1: the click_id and its comma (no bit from the newline on).
        let in_a = below(len);
        let (c_a, q_a) = (_mm256_and_si256(cut(commas, s), in_a), _mm256_and_si256(cut(quotes, s), in_a));
        let (c0, _) = lowest4(c_a);
        // Window 2, from after that comma: the user_id and its comma.
        let o1 = after(c0);
        let in_b = below(_mm256_sub_epi64(len, o1));
        let at_b = _mm256_add_epi64(s, o1);
        let (c_b, q_b) = (_mm256_and_si256(cut(commas, at_b), in_b), _mm256_and_si256(cut(quotes, at_b), in_b));
        let (c1, _) = lowest4(c_b);
        // Window 3, from after the second comma: one more comma and the newline.
        let o = _mm256_add_epi64(o1, after(c1));
        let nl = _mm256_sub_epi64(len, o);
        let in_c = below(nl);
        let at_c = _mm256_add_epi64(s, o);
        let (c_c, q_c) = (_mm256_and_si256(cut(commas, at_c), in_c), _mm256_and_si256(cut(quotes, at_c), in_c));
        let (c2, _) = lowest4(c_c);
        let shape = _mm256_cmplt_epu64_mask(c0, c64)
            & _mm256_cmplt_epu64_mask(c1, c64)
            & _mm256_cmpeq_epi64_mask(ones4(c_c), one)
            & _mm256_cmplt_epu64_mask(nl, c64);

        let q_a = _mm256_and_si256(q_a, below(c0));
        let q_b = _mm256_and_si256(q_b, below(c1));
        let opens = [_mm256_and_si256(q_a, one), _mm256_and_si256(q_b, one), _mm256_and_si256(q_c, one), bit4(q_c, after(c2))];
        let closes = [bit4(q_a, before(c0)), bit4(q_b, before(c1)), bit4(q_c, before(c2)), bit4(q_c, before(nl))];
        let mut f = opens[0];
        f = _mm256_or_si256(f, _mm256_slli_epi64::<1>(opens[1]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<2>(opens[2]));
        f = _mm256_or_si256(f, _mm256_slli_epi64::<3>(opens[3]));
        let mut l = closes[0];
        l = _mm256_or_si256(l, _mm256_slli_epi64::<1>(closes[1]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<2>(closes[2]));
        l = _mm256_or_si256(l, _mm256_slli_epi64::<3>(closes[3]));
        let quoted = _mm256_add_epi64(_mm256_add_epi64(opens[0], opens[1]), _mm256_add_epi64(opens[2], opens[3]));
        let count = _mm256_add_epi64(_mm256_add_epi64(ones4(q_a), ones4(q_b)), ones4(q_c));
        let sound = _mm256_cmpeq_epi64_mask(f, l) & _mm256_cmpeq_epi64_mask(count, _mm256_add_epi64(quoted, quoted));

        let odd = live & !(shape & sound);
        let take = if odd != 0 { live & _bzhi_u32(0x0F, odd.trailing_zeros()) as u8 } else { live };
        // No click_id, or no user_id (nothing, or just two quotes): left out.
        let empty = _mm256_cmpeq_epi64_mask(c0, _mm256_add_epi64(opens[0], opens[0]))
            | _mm256_cmpeq_epi64_mask(c1, _mm256_add_epi64(opens[1], opens[1]));
        let keep = take & !empty;
        let mut sep = _mm256_or_si256(c0, _mm256_slli_epi64::<8>(_mm256_add_epi64(o1, c1)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<16>(_mm256_add_epi64(o, c2)));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<24>(len));
        sep = _mm256_or_si256(sep, _mm256_slli_epi64::<56>(f));
        // (Compressed in a register and stored whole, eight lanes: there is
        // room for them, `n + 8 <= NB`. The compressing store to memory is
        // the same thing on Intel and many times slower on AMD's Zen 4.)
        _mm256_storeu_si256(st.p.as_mut_ptr().add(n) as *mut _, _mm256_maskz_compress_epi64(keep, _mm256_add_epi64(start0, s)));
        _mm256_storeu_si256(st.sep.as_mut_ptr().add(n) as *mut _, _mm256_maskz_compress_epi64(keep, sep));
        n += keep.count_ones() as usize;
        i += take.count_ones() as usize;
        if odd != 0 {
            stuck = true;
            break;
        }
        if take != 0x0F {
            break;
        }
    }
    (n, pos + (*ends.add(i)).wrapping_add(1) as usize, i, stuck | (i == 0))
}
