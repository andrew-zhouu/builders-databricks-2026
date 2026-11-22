//! The decode stage on processors with AVX-512 (`super::wide`): the same
//! results as `super::stage_decode`, eight lines at a time.
//!
//! Each 64-bit lane of a 512-bit register holds one line's value. A line's
//! bytes are fetched with gathers, eight at a time, from wherever its fields
//! are; everything after that is the one-line loop's arithmetic on eight
//! lanes. Nothing here is a branch on the data: all three timestamp forms are
//! computed and the right one is selected, where the one-line loop mispredicts
//! on every other line.
//!
//! What bounds this loop is the number of vector instructions per line, not
//! the gathers (seven fewer of them changed nothing): hence the full width.

use super::*;

/// Whether `id_trace8` computes the traces of eight event IDs in vector
/// registers (which must then mirror `super::id_trace` exactly; SS_SIMD_CHECK=1
/// compares them), or calls `super::id_trace` for each line.
const ID_TRACE_WIDE: bool = true;

const LOW: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;
/// Lines per step.
const W: usize = 8;

macro_rules! v {
    ($x:expr) => {
        _mm512_set1_epi64(($x) as i64)
    };
}
macro_rules! and {
    ($a:expr, $b:expr) => {
        _mm512_and_si512($a, $b)
    };
}
macro_rules! or {
    ($a:expr, $b:expr) => {
        _mm512_or_si512($a, $b)
    };
}
macro_rules! xor {
    ($a:expr, $b:expr) => {
        _mm512_xor_si512($a, $b)
    };
}
macro_rules! add {
    ($a:expr, $b:expr) => {
        _mm512_add_epi64($a, $b)
    };
}
macro_rules! sub {
    ($a:expr, $b:expr) => {
        _mm512_sub_epi64($a, $b)
    };
}
macro_rules! srl {
    ($a:expr, $n:literal) => {
        _mm512_srli_epi64::<$n>($a)
    };
}
macro_rules! sll {
    ($a:expr, $n:literal) => {
        _mm512_slli_epi64::<$n>($a)
    };
}
/// The product of the low 32 bits of each lane (both below 2^32 wherever this
/// is used, so it is the product).
macro_rules! mul {
    ($a:expr, $b:expr) => {
        _mm512_mul_epu32($a, $b)
    };
}
/// Eight bytes of each line, `$off` bytes from the lane's address.
macro_rules! gather {
    ($addr:expr, $off:expr) => {
        _mm512_i64gather_epi64::<1>($addr, ($off as isize) as *const i64)
    };
}

/// `super::id_trace` for eight lines: partitions (one per lane), first trace
/// words without the positions, and the lanes whose ID is not regular.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
#[inline]
unsafe fn id_trace8(idp: &IdParams, first: __m512i, rest: __m512i, n: __m512i) -> (__m512i, __m512i, __mmask8) {
    if ID_TRACE_WIDE {
        // ids::hash9, then as `id_trace`.
        let x = xor!(rest, _mm512_mullo_epi64(first, v!(0xD6E8_FEB8_6659_FD93u64)));
        let a = _mm512_mullo_epi64(x, v!(0x9E37_79B9_7F4A_7C15u64));
        let h = xor!(a, srl!(a, 32));
        let pk = or!(srl!(mul!(srl!(h, 32), v!(idp.pd)), 32), sll!(and!(srl!(h, 40), v!(15)), 16));
        let trace = _mm512_sll_epi64(and!(h, v!(idp.fp_mask)), _mm_cvtsi32_si128(idp.loc_bits as i32));
        (pk, trace, _mm512_cmpgt_epu64_mask(n, v!(9)))
    } else {
        let (mut f, mut r, mut len) = ([0u64; W], [0u64; W], [0u64; W]);
        _mm512_storeu_si512(f.as_mut_ptr() as *mut _, first);
        _mm512_storeu_si512(r.as_mut_ptr() as *mut _, rest);
        _mm512_storeu_si512(len.as_mut_ptr() as *mut _, n);
        let (mut pk, mut trace, mut dead) = ([0u64; W], [0u64; W], 0u8);
        for j in 0..W {
            let (p, t, d) = id_trace(idp, f[j] as u8, r[j], len[j]);
            pk[j] = p as u64;
            trace[j] = t;
            dead |= (d as u8) << j;
        }
        (_mm512_loadu_si512(pk.as_ptr() as *const _), _mm512_loadu_si512(trace.as_ptr() as *const _), dead)
    }
}

/// Decode the batch's lines: `super::stage_decode`, on this path.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
pub(super) unsafe fn decode(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    if !ctx.dates.wide_ok {
        // A window too long for the table of days by day of the month: the
        // ordinary decoder, which then sends every dated timestamp to the
        // scalar path.
        return stage_decode(ctx, st, n, loc0);
    }
    if wide_check() {
        return checked(ctx, st, n, loc0);
    }
    decode8(ctx, st, n, loc0)
}

#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
#[inline]
unsafe fn decode8(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    let dt = &ctx.dates;
    let idp = IdParams::new(ctx);
    let zero = _mm512_setzero_si512();
    let ones = v!(u64::MAX);
    let byte = v!(0xFF);
    let (ws, t0, span) = (v!(ctx.ws), v!(ctx.t0), v!((ctx.we - ctx.ws) as u64));
    let first_day = v!(dt.first_day_ms);
    let last_day = dt.day_text.len() as u64 - 1;
    // A position in the window's days, capped at the start of the last one
    // (see the division below).
    let day_cap = v!(last_day * DAY_MS as u64);
    let (ym_a, ym_b) = (v!(dt.ym[0]), v!(dt.ym[1]));
    let (days_a, days_b) = (v!(dt.ym_days[0]), v!(dt.ym_days[1]));
    let (base_a, base_b) = (v!(dt.ym_base[0]), v!(dt.ym_base[1]));
    // The day a date's text names, if it is one of the days of the table
    // (`DateTab::dom_text`): from the text's first eight bytes and the two
    // after them. The first eight must be one of the two months', the next
    // two digits, and the day they spell one of that month's in the table.
    macro_rules! day_of {
        ($q0:expr, $q1:expr) => {{
            let t = xor!(and!($q1, v!(0xFFFFu64)), v!(0x3030u64));
            let digits = _mm512_testn_epi64_mask(or!(add!(t, v!(0x7676u64)), t), v!(0x8080u64));
            let dd = add!(mul!(and!(t, byte), v!(10)), srl!(t, 8));
            let (ka, kb) = (_mm512_cmpeq_epi64_mask($q0, ym_a), _mm512_cmpeq_epi64_mask($q0, ym_b));
            let set = _mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(ka, days_a), kb, days_b);
            let base = _mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(ka, base_a), kb, base_b);
            (digits & _mm512_test_epi64_mask(_mm512_srlv_epi64(set, dd), v!(1)), add!(base, mul!(dd, v!(DAY_MS))))
        }};
    }
    let swap = _mm512_broadcast_i32x4(_mm_setr_epi8(7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8));
    let mut nmore = 0usize;
    let mut i = 0usize;
    while i < n {
        // The last group may be short: its missing lines are copies of its
        // first one (their results are stored like the others, past the end
        // of the batch, where nobody looks).
        let live: u8 = if n - i >= W { 0xFF } else { (1u8 << (n - i)) - 1 };
        let mut p = _mm512_loadu_si512(st.p.as_ptr().add(i) as *const _);
        let mut s = _mm512_loadu_si512(st.sep.as_ptr().add(i) as *const _);
        if live != 0xFF {
            p = _mm512_mask_blend_epi64(live, _mm512_broadcastq_epi64(_mm512_castsi512_si128(p)), p);
            s = _mm512_mask_blend_epi64(live, _mm512_broadcastq_epi64(_mm512_castsi512_si128(s)), s);
        }

        // `super::fields`: byte `k` of `start` is where field `k` starts in
        // its line, of `len` its length. (The quoted bits are bits 56 to 62.)
        let quoted = and!(_mm512_multishift_epi64_epi8(v!(0x3F3E_3D3C_3B3A_3938u64), s), v!(0x0001_0101_0101_0101u64));
        let seps = and!(s, v!(0x00FF_FFFF_FFFF_FFFFu64));
        let start = add!(add!(sll!(seps, 8), v!(0x0001_0101_0101_0100u64)), quoted);
        let len = sub!(sub!(seps, start), quoted);
        macro_rules! at {
            (0) => {
                add!(p, and!(start, byte))
            };
            ($k:literal) => {
                add!(p, and!(_mm512_srli_epi64::<{ 8 * $k }>(start), byte))
            };
        }
        macro_rules! ln {
            (0) => {
                and!(len, byte)
            };
            ($k:literal) => {
                and!(_mm512_srli_epi64::<{ 8 * $k }>(len), byte)
            };
        }

        // event_id: its first byte, and the rest (up to 8 bytes) zero-padded.
        let (a0, n0) = (at!(0), ln!(0));
        let first = srl!(gather!(a0, -7), 56);
        let rest = and!(gather!(a0, 1), _mm512_srlv_epi64(ones, sub!(v!(72), sll!(n0, 3))));
        let (pk, trace, dead_id) = id_trace8(&idp, first, rest, n0);
        _mm256_storeu_si256(st.pk.as_mut_ptr().add(i) as *mut __m256i, _mm512_cvtepi64_epi32(pk));
        _mm512_storeu_si512(st.trace.as_mut_ptr().add(i) as *mut _, or!(trace, add!(v!(loc0), p)));

        // user_id: `u` and 1 to 8 ASCII bytes, none of them NUL, is its own key.
        let (a1, n1) = (at!(1), ln!(1));
        let ufirst = srl!(gather!(a1, -7), 56);
        let raw = gather!(a1, 1);
        let keep1 = _mm512_srlv_epi64(ones, sub!(v!(72), sll!(n1, 3)));
        let odd = and!(and!(or!(_mm512_andnot_si512(raw, sub!(raw, v!(LOW))), raw), v!(HIGH)), keep1);
        let user_bad = _mm512_cmpneq_epi64_mask(odd, zero)
            | _mm512_cmpneq_epi64_mask(ufirst, v!(b'u'))
            | _mm512_cmpgt_epu64_mask(sub!(n1, v!(2)), v!(7));
        let key = _mm512_maskz_mov_epi64(!user_bad, _mm512_shuffle_epi8(and!(raw, keep1), swap));
        _mm512_storeu_si512(st.key.as_mut_ptr().add(i) as *mut _, key);
        for j in 0..W {
            *st.home.get_unchecked_mut(i + j) = ctx.table.prefetch_home(*st.key.get_unchecked(i + j)) as u32;
        }

        // ts: bytes 0 to 23 of the field, and eight more that depend on its
        // form: bytes 17 to 24 for F2 (its offset), the last eight otherwise
        // (the last one is looked at for a field of no known form).
        let (a2, n2) = (at!(2), ln!(2));
        let (g0, g1, g2) = (gather!(a2, 0), gather!(a2, 8), gather!(a2, 16));
        let kf1 = _mm512_cmpeq_epi64_mask(n2, v!(20));
        let kf2 = _mm512_cmpeq_epi64_mask(n2, v!(25));
        let kf3 = _mm512_cmplt_epu64_mask(sub!(n2, v!(1)), v!(16));
        let gx = gather!(_mm512_mask_mov_epi64(add!(a2, n2), kf2, add!(a2, v!(25))), -8);

        // F1 and F2 (see `super::event_ts`): the date is one of the table's
        // days, then `T`, then the time of day. (The table's unused entries
        // are sixteen 0xFF bytes at day start 0, which eleven 0xFF bytes for
        // a date match: so they do here.)
        let (in_table, day_ms) = day_of!(g0, g1);
        let all_ff = _mm512_cmpeq_epi64_mask(g0, ones)
            & _mm512_cmpeq_epi64_mask(and!(g1, v!(0xFF_FFFFu64)), v!(0xFF_FFFFu64));
        let date_ok = (in_table & _mm512_cmpeq_epi64_mask(and!(srl!(g1, 16), byte), v!(b'T'))) | all_ff;
        let d = xor!(or!(srl!(g1, 24), sll!(g2, 40)), v!(0x3030_3A30_303A_3030u64));
        let over = and!(or!(add!(d, v!(0x767A_7F76_7A7F_767Du64)), d), v!(HIGH));
        let y = add!(add!(d, sll!(d, 9)), sll!(d, 11)); // d * 0xA01
        let hh = and!(srl!(y, 8), byte);
        let secs = add!(add!(mul!(hh, v!(3600)), mul!(and!(srl!(y, 32), byte), v!(60))), srl!(y, 56));
        let ms1 = add!(_mm512_maskz_mov_epi64(!all_ff, day_ms), mul!(secs, v!(1000)));
        let bad1 = !date_ok | _mm512_cmpneq_epi64_mask(over, zero) | _mm512_cmpgt_epu64_mask(hh, v!(23));
        let sfx = and!(srl!(g2, 24), byte);
        // F2's offset: bytes 20 to 24 are bytes 3 to 7 of `gx`.
        let u = and!(_mm512_sub_epi8(gx, v!(0x3030_3A30_3000_0000u64)), v!(0xFFFF_FFFF_FF00_0000u64));
        let lim = v!(0x0909_0009_0900_0000u64);
        let y2 = add!(add!(u, sll!(u, 9)), sll!(u, 11));
        let (oh, om) = (and!(srl!(y2, 32), byte), srl!(y2, 56));
        let minus = _mm512_cmpeq_epi64_mask(sfx, v!(b'-'));
        let ok2 = (_mm512_cmpeq_epi64_mask(sfx, v!(b'+')) | minus)
            & _mm512_cmpeq_epi64_mask(_mm512_max_epu8(u, lim), lim)
            & _mm512_cmple_epu64_mask(oh, v!(23))
            & _mm512_cmple_epu64_mask(om, v!(59));
        let off = mul!(add!(mul!(oh, v!(60)), om), v!(60_000));
        let ms2 = _mm512_mask_add_epi64(sub!(ms1, off), minus, ms1, off);

        // F3 (see `super::digits16`): 1 to 16 digits. They are in `g0` and
        // `g1`; moved up by `16 - n2` bytes they are the sixteen bytes that
        // end where the field ends, with zeros before the field's start
        // (which, like whatever is really there, are not looked at).
        let up = sll!(sub!(v!(16), n2), 3);
        let hi8 = _mm512_sllv_epi64(g0, up);
        let lo8 = or!(
            _mm512_sllv_epi64(g1, up),
            or!(_mm512_srlv_epi64(g0, sub!(v!(64), up)), _mm512_sllv_epi64(g0, sub!(up, v!(64))))
        );
        let n2lo = _mm512_min_epu64(n2, v!(8));
        let keep_lo = _mm512_sllv_epi64(ones, sub!(v!(64), sll!(n2lo, 3)));
        let keep_hi = _mm512_sllv_epi64(ones, sub!(v!(64), sll!(sub!(n2, n2lo), 3)));
        let (c0, c9) = (_mm512_set1_epi8(b'0' as i8), _mm512_set1_epi8(9));
        let (th, tl) = (_mm512_sub_epi8(hi8, c0), _mm512_sub_epi8(lo8, c0));
        let not_digit = or!(
            _mm512_maskz_mov_epi8(_mm512_cmpgt_epu8_mask(th, c9), keep_hi),
            _mm512_maskz_mov_epi8(_mm512_cmpgt_epu8_mask(tl, c9), keep_lo)
        );
        let (w10, w100) = (_mm512_set1_epi16(0x010A), _mm512_set1_epi32(0x0001_0064));
        let qh = _mm512_madd_epi16(_mm512_maddubs_epi16(and!(th, keep_hi), w10), w100);
        let ql = _mm512_madd_epi16(_mm512_maddubs_epi16(and!(tl, keep_lo), w10), w100);
        let vh = add!(mul!(qh, v!(10_000)), srl!(qh, 32));
        let vl = add!(mul!(ql, v!(10_000)), srl!(ql, 32));
        let ms3 = add!(mul!(vh, v!(100_000_000)), vl);

        // Which form it is, and what that makes of the line.
        let kf12 = kf1 | kf2;
        let maybe_digits = _mm512_cmpneq_epi64_mask(n2, zero)
            & _mm512_cmplt_epu64_mask(sub!(and!(g0, byte), v!(b'0')), v!(10))
            & _mm512_cmplt_epu64_mask(sub!(srl!(gx, 56), v!(b'0')), v!(10));
        let ts_dead = (kf12 & bad1) | (!(kf12 | kf3) & maybe_digits);
        let ts_ok = (kf1 & !bad1 & _mm512_cmpeq_epi64_mask(sfx, v!(b'Z')))
            | (kf2 & !bad1 & ok2)
            | (kf3 & _mm512_testn_epi64_mask(not_digit, not_digit));
        let ms = _mm512_mask_mov_epi64(_mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(kf3, ms3), kf1, ms1), kf2, ms2);
        let in_window = _mm512_cmplt_epu64_mask(sub!(ms, ws), span);
        _mm256_storeu_si256(st.ts.as_mut_ptr().add(i) as *mut __m256i, _mm512_cvtepi64_epi32(sub!(ms, t0)));

        // ingest_ts on the same UTC day as ts? `super::stage_decode` compares
        // its first ten bytes with the text of the day of ts; for an event
        // that is valid (the flag is not used otherwise), that is: its date
        // is one of the table's days, the one `ms` falls on. The window's day
        // of `ms` is `(ms - first day) / DAY_MS`; capped, the dividend is
        // below 2^32, and for such `x`, `x / DAY_MS` is `(x >> 10) / 84375`,
        // which is `((x >> 10) * 3335999724) >> 48` exactly.
        let (a3, n3) = (at!(3), ln!(3));
        let (i0, i8, i12) = (gather!(a3, 0), gather!(a3, 8), gather!(a3, 12));
        let x = _mm512_min_epu64(sub!(ms, first_day), day_cap);
        let day = srl!(mul!(srl!(x, 10), v!(3_335_999_724u64)), 48);
        let (ingest_in_table, ingest_day_ms) = day_of!(i0, i8);
        let same_day = ingest_in_table
            & _mm512_cmpeq_epi64_mask(ingest_day_ms, add!(first_day, mul!(day, v!(DAY_MS))))
            & _mm512_cmpeq_epi64_mask(srl!(i12, 56), v!(b'Z'));

        // view / cart / purchase, exactly; a product.
        let (a4, n4) = (at!(4), ln!(4));
        let w = gather!(a4, 0);
        let wl = and!(w, v!(0xFFFF_FFFFu64));
        let k_purchase = _mm512_cmpeq_epi64_mask(n4, v!(8)) & _mm512_cmpeq_epi64_mask(w, v!(PURCHASE));
        let k_view = _mm512_cmpeq_epi64_mask(n4, v!(4))
            & (_mm512_cmpeq_epi64_mask(wl, v!(VIEW)) | _mm512_cmpeq_epi64_mask(wl, v!(CART)));
        let has_product = _mm512_cmpneq_epi64_mask(ln!(5), zero);

        // The flags, as `super::stage_decode` sets them.
        let dead = dead_id
            | _mm512_cmpneq_epi64_mask(n3, v!(20))
            | ts_dead
            | (user_bad & _mm512_cmpneq_epi64_mask(n1, zero));
        let valid = !(dead | user_bad) & (k_purchase | k_view) & has_product & ts_ok & in_window;
        let ingest = valid & !same_day;
        let purchase = valid & k_purchase;
        let fl = or!(
            or!(_mm512_maskz_mov_epi64(dead, v!(F_DEAD)), _mm512_maskz_mov_epi64(valid, v!(F_VALID))),
            or!(_mm512_maskz_mov_epi64(purchase, v!(F_PURCHASE)), _mm512_maskz_mov_epi64(ingest, v!(F_INGEST)))
        );
        (st.flags.as_mut_ptr().add(i) as *mut u64).write_unaligned(_mm_cvtsi128_si64(_mm512_cvtepi64_epi8(fl)) as u64);
        // The lines with more to do, in order: their numbers picked out of
        // the eight packed in a word.
        let mk = ((ingest | purchase) & live) as u64;
        let numbers = 0x0706_0504_0302_0100u64 + LOW * i as u64;
        (st.more.as_mut_ptr().add(nmore) as *mut u64).write_unaligned(_pext_u64(numbers, _pdep_u64(mk, LOW) * 0xFF));
        nmore += mk.count_ones() as usize;
        i += W;
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
    let nw = decode8(ctx, st, n, loc0);
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
