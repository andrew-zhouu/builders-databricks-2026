//! The decode stage on processors with AVX-512 (`super::wide`): the same
//! results as `super::stage_decode`, four lines at a time.
//!
//! Each 64-bit lane of a 256-bit register holds one line's value. A line's
//! bytes are fetched with gathers, eight at a time, from wherever its fields
//! are; everything after that is the one-line loop's arithmetic on four lanes.
//! Nothing here is a branch on the data: all three timestamp forms are
//! computed and the right one is selected, where the one-line loop mispredicts
//! on every other line.
//!
//! The registers stay 256 bits wide. (512-bit registers lower the clock even
//! on the processors that take this path, and the gathers, which set the pace,
//! would not get faster.)

use super::*;

/// Whether `id_trace4` computes the traces of four event IDs in vector
/// registers (which must then mirror `super::id_trace` exactly; SS_SIMD_CHECK=1
/// compares them), or calls `super::id_trace` for each line.
const ID_TRACE_WIDE: bool = true;

const LOW: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

macro_rules! v {
    ($x:expr) => {
        _mm256_set1_epi64x(($x) as i64)
    };
}
macro_rules! and {
    ($a:expr, $b:expr) => {
        _mm256_and_si256($a, $b)
    };
}
macro_rules! or {
    ($a:expr, $b:expr) => {
        _mm256_or_si256($a, $b)
    };
}
macro_rules! xor {
    ($a:expr, $b:expr) => {
        _mm256_xor_si256($a, $b)
    };
}
macro_rules! add {
    ($a:expr, $b:expr) => {
        _mm256_add_epi64($a, $b)
    };
}
macro_rules! sub {
    ($a:expr, $b:expr) => {
        _mm256_sub_epi64($a, $b)
    };
}
macro_rules! srl {
    ($a:expr, $n:literal) => {
        _mm256_srli_epi64::<$n>($a)
    };
}
macro_rules! sll {
    ($a:expr, $n:literal) => {
        _mm256_slli_epi64::<$n>($a)
    };
}
/// The product of the low 32 bits of each lane (both below 2^32 wherever this
/// is used, so it is the product).
macro_rules! mul {
    ($a:expr, $b:expr) => {
        _mm256_mul_epu32($a, $b)
    };
}
/// Eight bytes of each line, `$off` bytes from the lane's address.
macro_rules! gather {
    ($addr:expr, $off:expr) => {
        _mm256_i64gather_epi64::<1>(($off as isize) as *const i64, $addr)
    };
}

/// `super::id_trace` for four lines: partitions (one per lane), first trace
/// words without the positions, and the lanes whose ID is not regular.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
#[inline]
unsafe fn id_trace4(idp: &IdParams, first: __m256i, rest: __m256i, n: __m256i) -> (__m256i, __m256i, __mmask8) {
    if ID_TRACE_WIDE {
        // ids::hash9, then as `id_trace`.
        let x = xor!(rest, _mm256_mullo_epi64(first, v!(0xD6E8_FEB8_6659_FD93u64)));
        let a = _mm256_mullo_epi64(x, v!(0x9E37_79B9_7F4A_7C15u64));
        let h = xor!(a, srl!(a, 32));
        let pk = or!(srl!(mul!(srl!(h, 32), v!(idp.pd)), 32), sll!(and!(srl!(h, 40), v!(15)), 16));
        let trace = _mm256_sll_epi64(and!(h, v!(idp.fp_mask)), _mm_cvtsi32_si128(idp.loc_bits as i32));
        (pk, trace, _mm256_cmpgt_epu64_mask(n, v!(9)))
    } else {
        let (mut f, mut r, mut len) = ([0u64; 4], [0u64; 4], [0u64; 4]);
        _mm256_storeu_si256(f.as_mut_ptr() as *mut __m256i, first);
        _mm256_storeu_si256(r.as_mut_ptr() as *mut __m256i, rest);
        _mm256_storeu_si256(len.as_mut_ptr() as *mut __m256i, n);
        let (mut pk, mut trace, mut dead) = ([0u64; 4], [0u64; 4], 0u8);
        for j in 0..4 {
            let (p, t, d) = id_trace(idp, f[j] as u8, r[j], len[j]);
            pk[j] = p as u64;
            trace[j] = t;
            dead |= (d as u8) << j;
        }
        (_mm256_loadu_si256(pk.as_ptr() as *const __m256i), _mm256_loadu_si256(trace.as_ptr() as *const __m256i), dead)
    }
}

/// For each set of lanes: 0xFF in the bytes of those lanes' numbers, for
/// picking them out of a packed word.
static PICK: [u32; 16] = {
    let mut t = [0u32; 16];
    let mut m = 0;
    while m < 16 {
        let mut j = 0;
        while j < 4 {
            if m >> j & 1 != 0 {
                t[m] |= 0xFF << (8 * j);
            }
            j += 1;
        }
        m += 1;
    }
    t
};

/// Decode the batch's lines: `super::stage_decode`, on this path.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
pub(super) unsafe fn decode(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    if wide_check() {
        return checked(ctx, st, n, loc0);
    }
    decode4(ctx, st, n, loc0)
}

#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
#[inline]
unsafe fn decode4(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    let dt = &ctx.dates;
    let idp = IdParams::new(ctx);
    let zero = _mm256_setzero_si256();
    let ones = v!(u64::MAX);
    let byte = v!(0xFF);
    let (ws, t0, span) = (v!(ctx.ws), v!(ctx.t0), v!((ctx.we - ctx.ws) as u64));
    let first_day = v!(dt.first_day_ms);
    let last_day = dt.day_text.len() as u64 - 1;
    // A position in the window's days, capped at the start of the last one
    // (see the division below).
    let day_cap = v!(last_day * DAY_MS as u64);
    let dom_text = dt.dom_text.as_ptr() as *const i64;
    let dom_ms = dt.dom_ms.as_ptr();
    let day_text = dt.day_text.as_ptr() as *const i64;
    let swap = _mm256_setr_epi8(
        7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8,
    );
    let mut nmore = 0usize;
    let mut i = 0usize;
    while i < n {
        // The last group may be short: its missing lines are copies of its
        // first one (their results are stored like the others, past the end
        // of the batch, where nobody looks).
        let live: u8 = if n - i >= 4 { 0xF } else { (1u8 << (n - i)) - 1 };
        let mut p = _mm256_loadu_si256(st.p.as_ptr().add(i) as *const __m256i);
        let mut s = _mm256_loadu_si256(st.sep.as_ptr().add(i) as *const __m256i);
        if live != 0xF {
            p = _mm256_mask_blend_epi64(live, _mm256_broadcastq_epi64(_mm256_castsi256_si128(p)), p);
            s = _mm256_mask_blend_epi64(live, _mm256_broadcastq_epi64(_mm256_castsi256_si128(s)), s);
        }

        // `super::fields`: byte `k` of `start` is where field `k` starts in
        // its line, of `len` its length. (The quoted bits are bits 56 to 62.)
        let quoted = and!(_mm256_multishift_epi64_epi8(v!(0x3F3E_3D3C_3B3A_3938u64), s), v!(0x0001_0101_0101_0101u64));
        let seps = and!(s, v!(0x00FF_FFFF_FFFF_FFFFu64));
        let start = add!(add!(sll!(seps, 8), v!(0x0001_0101_0101_0100u64)), quoted);
        let len = sub!(sub!(seps, start), quoted);
        macro_rules! at {
            (0) => {
                add!(p, and!(start, byte))
            };
            ($k:literal) => {
                add!(p, and!(_mm256_srli_epi64::<{ 8 * $k }>(start), byte))
            };
        }
        macro_rules! ln {
            (0) => {
                and!(len, byte)
            };
            ($k:literal) => {
                and!(_mm256_srli_epi64::<{ 8 * $k }>(len), byte)
            };
        }

        // event_id: its first byte, and the rest (up to 8 bytes) zero-padded.
        let (a0, n0) = (at!(0), ln!(0));
        let first = srl!(gather!(a0, -7), 56);
        let rest = and!(gather!(a0, 1), _mm256_srlv_epi64(ones, sub!(v!(72), sll!(n0, 3))));
        let (pk, trace, dead_id) = id_trace4(&idp, first, rest, n0);
        _mm_storeu_si128(st.pk.as_mut_ptr().add(i) as *mut __m128i, _mm256_cvtepi64_epi32(pk));
        _mm256_storeu_si256(st.trace.as_mut_ptr().add(i) as *mut __m256i, or!(trace, add!(v!(loc0), p)));

        // user_id: `u` and 1 to 8 ASCII bytes, none of them NUL, is its own key.
        let (a1, n1) = (at!(1), ln!(1));
        let ufirst = srl!(gather!(a1, -7), 56);
        let raw = gather!(a1, 1);
        let keep1 = _mm256_srlv_epi64(ones, sub!(v!(72), sll!(n1, 3)));
        let odd = and!(and!(or!(_mm256_andnot_si256(raw, sub!(raw, v!(LOW))), raw), v!(HIGH)), keep1);
        let user_bad = _mm256_cmpneq_epi64_mask(odd, zero)
            | _mm256_cmpneq_epi64_mask(ufirst, v!(b'u'))
            | _mm256_cmpgt_epu64_mask(sub!(n1, v!(2)), v!(7));
        let key = _mm256_maskz_mov_epi64(!user_bad, _mm256_shuffle_epi8(and!(raw, keep1), swap));
        _mm256_storeu_si256(st.key.as_mut_ptr().add(i) as *mut __m256i, key);
        for j in 0..4 {
            *st.home.get_unchecked_mut(i + j) = ctx.table.prefetch_home(*st.key.get_unchecked(i + j)) as u32;
        }

        // ts: bytes 0..24 of the field, and the 16 bytes that end where it ends.
        let (a2, n2) = (at!(2), ln!(2));
        let (g0, g1, g2, g17) = (gather!(a2, 0), gather!(a2, 8), gather!(a2, 16), gather!(a2, 17));
        let end = add!(a2, n2);
        let (hi8, lo8) = (gather!(end, -16), gather!(end, -8));
        let kf1 = _mm256_cmpeq_epi64_mask(n2, v!(20));
        let kf2 = _mm256_cmpeq_epi64_mask(n2, v!(25));
        let kf3 = _mm256_cmplt_epu64_mask(sub!(n2, v!(1)), v!(16));

        // F1 and F2 (see `super::event_ts`): the day by its day of the month,
        // the text of the date compared, the time of day.
        let d8 = and!(g1, v!(15));
        let dom = add!(add!(sll!(d8, 3), sll!(d8, 1)), and!(srl!(g1, 8), v!(15)));
        let dom2 = sll!(dom, 1);
        let date_ok = _mm256_cmpeq_epi64_mask(g0, _mm256_i64gather_epi64::<8>(dom_text, dom2))
            & _mm256_testn_epi64_mask(xor!(g1, _mm256_i64gather_epi64::<8>(dom_text.add(1), dom2)), v!(0xFF_FFFFu64));
        let d = xor!(or!(srl!(g1, 24), sll!(g2, 40)), v!(0x3030_3A30_303A_3030u64));
        let over = and!(or!(add!(d, v!(0x767A_7F76_7A7F_767Du64)), d), v!(HIGH));
        let y = add!(add!(d, sll!(d, 9)), sll!(d, 11)); // d * 0xA01
        let hh = and!(srl!(y, 8), byte);
        let secs = add!(add!(mul!(hh, v!(3600)), mul!(and!(srl!(y, 32), byte), v!(60))), srl!(y, 56));
        let ms1 = add!(_mm256_i64gather_epi64::<8>(dom_ms, dom), mul!(secs, v!(1000)));
        let bad1 = !date_ok | _mm256_cmpneq_epi64_mask(over, zero) | _mm256_cmpgt_epu64_mask(hh, v!(23));
        let sfx = and!(srl!(g2, 24), byte);
        // F2's offset: bytes 20 to 24 are bytes 3 to 7 of `g17`.
        let u = and!(_mm256_sub_epi8(g17, v!(0x3030_3A30_3000_0000u64)), v!(0xFFFF_FFFF_FF00_0000u64));
        let lim = v!(0x0909_0009_0900_0000u64);
        let y2 = add!(add!(u, sll!(u, 9)), sll!(u, 11));
        let (oh, om) = (and!(srl!(y2, 32), byte), srl!(y2, 56));
        let minus = _mm256_cmpeq_epi64_mask(sfx, v!(b'-'));
        let ok2 = (_mm256_cmpeq_epi64_mask(sfx, v!(b'+')) | minus)
            & _mm256_cmpeq_epi64_mask(_mm256_max_epu8(u, lim), lim)
            & _mm256_cmple_epu64_mask(oh, v!(23))
            & _mm256_cmple_epu64_mask(om, v!(59));
        let off = mul!(add!(mul!(oh, v!(60)), om), v!(60_000));
        let ms2 = _mm256_mask_add_epi64(sub!(ms1, off), minus, ms1, off);

        // F3 (see `super::digits16`): the last `n2` of the 16 bytes are digits.
        let n2lo = _mm256_min_epu64(n2, v!(8));
        let keep_lo = _mm256_sllv_epi64(ones, sub!(v!(64), sll!(n2lo, 3)));
        let keep_hi = _mm256_sllv_epi64(ones, sub!(v!(64), sll!(sub!(n2, n2lo), 3)));
        let (c0, c9) = (_mm256_set1_epi8(b'0' as i8), _mm256_set1_epi8(9));
        let (th, tl) = (_mm256_sub_epi8(hi8, c0), _mm256_sub_epi8(lo8, c0));
        let not_digit = or!(
            _mm256_andnot_si256(_mm256_cmpeq_epi8(_mm256_min_epu8(th, c9), th), keep_hi),
            _mm256_andnot_si256(_mm256_cmpeq_epi8(_mm256_min_epu8(tl, c9), tl), keep_lo)
        );
        let (w10, w100) = (_mm256_set1_epi16(0x010A), _mm256_set1_epi32(0x0001_0064));
        let qh = _mm256_madd_epi16(_mm256_maddubs_epi16(and!(th, keep_hi), w10), w100);
        let ql = _mm256_madd_epi16(_mm256_maddubs_epi16(and!(tl, keep_lo), w10), w100);
        let vh = add!(mul!(qh, v!(10_000)), srl!(qh, 32));
        let vl = add!(mul!(ql, v!(10_000)), srl!(ql, 32));
        let ms3 = add!(mul!(vh, v!(100_000_000)), vl);

        // Which form it is, and what that makes of the line.
        let kf12 = kf1 | kf2;
        let maybe_digits = _mm256_cmpneq_epi64_mask(n2, zero)
            & _mm256_cmplt_epu64_mask(sub!(and!(g0, byte), v!(b'0')), v!(10))
            & _mm256_cmplt_epu64_mask(sub!(srl!(lo8, 56), v!(b'0')), v!(10));
        let ts_dead = (kf12 & bad1) | (!(kf12 | kf3) & maybe_digits);
        let ts_ok = (kf1 & !bad1 & _mm256_cmpeq_epi64_mask(sfx, v!(b'Z')))
            | (kf2 & !bad1 & ok2)
            | (kf3 & _mm256_testn_epi64_mask(not_digit, not_digit));
        let ms = _mm256_mask_mov_epi64(_mm256_mask_mov_epi64(_mm256_maskz_mov_epi64(kf3, ms3), kf1, ms1), kf2, ms2);
        let in_window = _mm256_cmplt_epu64_mask(sub!(ms, ws), span);
        _mm_storeu_si128(st.ts.as_mut_ptr().add(i) as *mut __m128i, _mm256_cvtepi64_epi32(sub!(ms, t0)));

        // ingest_ts on the same UTC day as ts? Its first ten bytes against the
        // day of ts as text: ts's own text when that is F1, else the text of
        // the window's day `(ms - first day) / DAY_MS`. Capped, the dividend is
        // below 2^32, and for such `x`, `x / DAY_MS` is `(x >> 10) / 84375`,
        // which is `((x >> 10) * 3335999724) >> 48` exactly.
        let (a3, n3) = (at!(3), ln!(3));
        let (i0, i8, i12) = (gather!(a3, 0), gather!(a3, 8), gather!(a3, 12));
        let x = _mm256_min_epu64(sub!(ms, first_day), day_cap);
        let day2 = sll!(srl!(mul!(srl!(x, 10), v!(3_335_999_724u64)), 48), 1);
        let not_f1 = _mm256_movm_epi64(!kf1);
        let dq0 = _mm256_mask_i64gather_epi64::<8>(g0, day_text, day2, not_f1);
        let dq1 = _mm256_mask_i64gather_epi64::<8>(g1, day_text.add(1), day2, not_f1);
        let same_day = _mm256_cmpeq_epi64_mask(i0, dq0)
            & _mm256_testn_epi64_mask(xor!(i8, dq1), v!(0xFFFFu64))
            & _mm256_cmpeq_epi64_mask(srl!(i12, 56), v!(b'Z'));

        // view / cart / purchase, exactly; a product.
        let (a4, n4) = (at!(4), ln!(4));
        let w = gather!(a4, 0);
        let wl = and!(w, v!(0xFFFF_FFFFu64));
        let k_purchase = _mm256_cmpeq_epi64_mask(n4, v!(8)) & _mm256_cmpeq_epi64_mask(w, v!(PURCHASE));
        let k_view = _mm256_cmpeq_epi64_mask(n4, v!(4))
            & (_mm256_cmpeq_epi64_mask(wl, v!(VIEW)) | _mm256_cmpeq_epi64_mask(wl, v!(CART)));
        let has_product = _mm256_cmpneq_epi64_mask(ln!(5), zero);

        // The flags, as `super::stage_decode` sets them.
        let dead = (dead_id
            | _mm256_cmpneq_epi64_mask(n3, v!(20))
            | ts_dead
            | (user_bad & _mm256_cmpneq_epi64_mask(n1, zero)))
            & 0xF;
        let valid = !(dead | user_bad) & (k_purchase | k_view) & has_product & ts_ok & in_window & 0xF;
        let ingest = valid & !same_day;
        let purchase = valid & k_purchase;
        let fl = or!(
            or!(_mm256_maskz_mov_epi64(dead, v!(F_DEAD)), _mm256_maskz_mov_epi64(valid, v!(F_VALID))),
            or!(_mm256_maskz_mov_epi64(purchase, v!(F_PURCHASE)), _mm256_maskz_mov_epi64(ingest, v!(F_INGEST)))
        );
        (st.flags.as_mut_ptr().add(i) as *mut u32).write_unaligned(_mm_cvtsi128_si32(_mm256_cvtepi64_epi8(fl)) as u32);
        // The lines with more to do, in order: their numbers picked out of
        // the four packed in a word.
        let mk = ((ingest | purchase) & live) as usize;
        let numbers = 0x0302_0100u32 + 0x0101_0101 * i as u32;
        (st.more.as_mut_ptr().add(nmore) as *mut u32).write_unaligned(_pext_u32(numbers, *PICK.get_unchecked(mk)));
        nmore += mk.count_ones() as usize;
        i += 4;
    }
    nmore
}

/// SS_SIMD_CHECK=1: the batch through both paths. The split of every line
/// again by the ordinary splitter, then both decoders; any difference that the
/// later stages could see stops the run with the line.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
unsafe fn checked(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    let (lines, seps) = (st.p, st.sep);
    let fail = |i: usize, what: &str, a: u64, b: u64| -> ! {
        let line = std::slice::from_raw_parts(lines[i], (seps[i] >> 48 & 0xFF) as usize);
        eprintln!(
            "SS_SIMD_CHECK: {what} differs on line {i} of a batch of {n}: wide {a:#x}, ordinary {b:#x}\n{}",
            String::from_utf8_lossy(line)
        );
        std::process::abort()
    };
    let mut one = Stage::new();
    for i in 0..n {
        let (used, added) = split_event(&mut one, 0, st.p[i]);
        if !added || used != (st.sep[i] >> 48 & 0xFF) as usize + 1 {
            fail(i, "split", st.sep[i], used as u64);
        }
        if one.sep[0] != st.sep[i] {
            fail(i, "separators", st.sep[i], one.sep[0]);
        }
    }
    let mut base = *st;
    let nb = stage_decode(ctx, &mut base, n, loc0);
    let nw = decode4(ctx, st, n, loc0);
    for i in 0..n {
        let (fw, fb) = (st.flags[i], base.flags[i]);
        if fw != fb {
            fail(i, "flags", fw as u64, fb as u64);
        }
        if fw & F_DEAD != 0 {
            continue;
        }
        if st.pk[i] != base.pk[i] {
            fail(i, "partition", st.pk[i] as u64, base.pk[i] as u64);
        }
        if st.trace[i] != base.trace[i] {
            fail(i, "trace", st.trace[i], base.trace[i]);
        }
        if fw & F_VALID != 0 {
            if st.key[i] != base.key[i] {
                fail(i, "user key", st.key[i], base.key[i]);
            }
            if st.home[i] != base.home[i] {
                fail(i, "home slot", st.home[i] as u64, base.home[i] as u64);
            }
            if st.ts[i] != base.ts[i] {
                fail(i, "timestamp", st.ts[i] as u64, base.ts[i] as u64);
            }
        }
    }
    if nw != nb || st.more[..nw] != base.more[..nb] {
        fail(0, "list of lines with more to do", nw as u64, nb as u64);
    }
    nw
}
