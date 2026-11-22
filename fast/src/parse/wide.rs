//! The decode stages on processors with AVX-512 (`super::wide_kind`): the same
//! results as `super::stage_decode` and `super::click_decode`, eight lines at
//! a time. Nothing here needs more than AVX-512 F, BW, VL and DQ, so both wide
//! parser instances use this code: the one for processors with VBMI2 and the
//! one for those without.
//!
//! Each 64-bit lane of a 512-bit register holds one line's value. A line's
//! bytes are fetched with gathers, eight at a time, from wherever its fields
//! are; everything after that is the one-line loops' arithmetic on eight
//! lanes. Nothing here is a branch on the data: all three timestamp forms are
//! computed and the right one is selected, where the one-line loops mispredict
//! on every other line.
//!
//! What bounds these loops is the number of vector instructions per line, not
//! the gathers (seven fewer of them changed nothing): hence the full width.

use super::*;

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
/// The same for the lanes of `$k` only (zero in the others, whose memory is
/// not touched).
macro_rules! gather_if {
    ($k:expr, $addr:expr, $off:expr) => {
        _mm512_mask_i64gather_epi64::<1>(_mm512_setzero_si512(), $k, $addr, ($off as isize) as *const i64)
    };
}
/// The batch's lines `$i..$i + 8` of a stage array, with the lines past the
/// end of the batch (`$live` has a bit for each line there is) replaced by
/// copies of line `$i`: their results are stored like the others, where
/// nobody looks.
macro_rules! lines {
    ($array:expr, $i:expr, $live:expr) => {{
        let x = _mm512_loadu_si512($array.as_ptr().add($i) as *const _);
        if $live != 0xFF {
            _mm512_mask_blend_epi64($live, _mm512_broadcastq_epi64(_mm512_castsi512_si128(x)), x)
        } else {
            x
        }
    }};
}
/// Field `$k` of each line: where it starts (an address) and its length, from
/// `fields8`'s two words and the lines' addresses.
macro_rules! field {
    ($p:expr, $start:expr, $len:expr, 0) => {
        (add!($p, and!($start, v!(0xFF))), and!($len, v!(0xFF)))
    };
    ($p:expr, $start:expr, $len:expr, $k:literal) => {
        (
            add!($p, and!(_mm512_srli_epi64::<{ 8 * $k }>($start), v!(0xFF))),
            and!(_mm512_srli_epi64::<{ 8 * $k }>($len), v!(0xFF)),
        )
    };
}

/// `super::fields` for eight lines: byte `k` of the first result is where
/// field `k` starts in its line, of the second its length. (The quoted bits
/// are bits 56 to 62 of `sep`.)
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn fields8(sep: __m512i) -> (__m512i, __m512i) {
    // One byte per field, 1 if it is quoted: the top byte of `sep` in every
    // byte of its lane, bit `k` of it kept in byte `k`, and what is left
    // made 1. (Three instructions that every processor with AVX-512 BW has.)
    let top = _mm512_shuffle_epi8(sep, _mm512_broadcast_i32x4(_mm_setr_epi8(7, 7, 7, 7, 7, 7, 7, 7, 15, 15, 15, 15, 15, 15, 15, 15)));
    let quoted = _mm512_min_epu8(and!(top, v!(0x0040_2010_0804_0201u64)), v!(0x0001_0101_0101_0101u64));
    let seps = and!(sep, v!(0x00FF_FFFF_FFFF_FFFFu64));
    let start = add!(add!(sll!(seps, 8), v!(0x0001_0101_0101_0100u64)), quoted);
    (start, sub!(sub!(seps, start), quoted))
}

/// An ID of the regular shape (`ids::raw_key`): the letter `first`, then 1 to
/// 8 ASCII bytes, none of them NUL. Returns the lanes whose field (`n` bytes
/// at `a`) is not of that shape, and the key the bytes after the letter make
/// (meaningful for the lanes that are).
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn key8(a: __m512i, n: __m512i, first: u8) -> (__mmask8, __m512i) {
    let letter = srl!(gather!(a, -7), 56);
    let raw = gather!(a, 1);
    let keep = _mm512_srlv_epi64(v!(u64::MAX), sub!(v!(72), sll!(n, 3)));
    let odd = and!(and!(or!(_mm512_andnot_si512(raw, sub!(raw, v!(LOW))), raw), v!(HIGH)), keep);
    let bad = _mm512_cmpneq_epi64_mask(odd, _mm512_setzero_si512())
        | _mm512_cmpneq_epi64_mask(letter, v!(first))
        | _mm512_cmpgt_epu64_mask(sub!(n, v!(2)), v!(7));
    let swap = _mm512_broadcast_i32x4(_mm_setr_epi8(7, 6, 5, 4, 3, 2, 1, 0, 15, 14, 13, 12, 11, 10, 9, 8));
    (bad, _mm512_shuffle_epi8(and!(raw, keep), swap))
}

/// The days of `DateTab::dom_text`, in registers.
struct Days {
    ym: [__m512i; 2],
    days: [__m512i; 2],
    base: [__m512i; 2],
}

#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn days(dt: &DateTab) -> Days {
    Days {
        ym: [v!(dt.ym[0]), v!(dt.ym[1])],
        days: [v!(dt.ym_days[0]), v!(dt.ym_days[1])],
        base: [v!(dt.ym_base[0]), v!(dt.ym_base[1])],
    }
}

/// The day a date's text names, if it is one of the days of the table
/// (`DateTab::dom_text`): from the text's first eight bytes and the two after
/// them (the low bytes of `q1`). The first eight must be one of the two
/// months', the next two digits, and the day they spell one of that month's
/// in the table. Returns the lanes for which that is so, and the epoch ms of
/// the day's start.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn day_of(d: &Days, q0: __m512i, q1: __m512i) -> (__mmask8, __m512i) {
    let t = xor!(and!(q1, v!(0xFFFFu64)), v!(0x3030u64));
    let digits = _mm512_testn_epi64_mask(or!(add!(t, v!(0x7676u64)), t), v!(0x8080u64));
    let dd = add!(mul!(and!(t, v!(0xFF)), v!(10)), srl!(t, 8));
    let (ka, kb) = (_mm512_cmpeq_epi64_mask(q0, d.ym[0]), _mm512_cmpeq_epi64_mask(q0, d.ym[1]));
    let set = _mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(ka, d.days[0]), kb, d.days[1]);
    let base = _mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(ka, d.base[0]), kb, d.base[1]);
    (digits & _mm512_test_epi64_mask(_mm512_srlv_epi64(set, dd), v!(1)), add!(base, mul!(dd, v!(DAY_MS))))
}

/// `super::event_ts` for eight `ts` fields of `n` bytes at `a`: the lanes the
/// scalar path must decide, the lanes with a valid timestamp, and the epoch
/// ms (meaningful for those).
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn ts8(d: &Days, a: __m512i, n: __m512i) -> (__mmask8, __mmask8, __m512i) {
    let zero = _mm512_setzero_si512();
    let ones = v!(u64::MAX);
    let byte = v!(0xFF);
    // Bytes 0 to 23 of the field, and eight more that depend on its form:
    // bytes 17 to 24 for F2 (its offset), the last eight otherwise (the last
    // one is looked at for a field of no known form).
    let (g0, g1, g2) = (gather!(a, 0), gather!(a, 8), gather!(a, 16));
    let kf1 = _mm512_cmpeq_epi64_mask(n, v!(20));
    let kf2 = _mm512_cmpeq_epi64_mask(n, v!(25));
    let kf3 = _mm512_cmplt_epu64_mask(sub!(n, v!(1)), v!(16));
    let gx = gather!(_mm512_mask_mov_epi64(add!(a, n), kf2, add!(a, v!(25))), -8);

    // F1 and F2: the date is one of the table's days, then `T`, then the
    // time of day. (The table's unused entries are sixteen 0xFF bytes at day
    // start 0, which eleven 0xFF bytes for a date match: so they do here.)
    let (in_table, day_ms) = day_of(d, g0, g1);
    let all_ff =
        _mm512_cmpeq_epi64_mask(g0, ones) & _mm512_cmpeq_epi64_mask(and!(g1, v!(0xFF_FFFFu64)), v!(0xFF_FFFFu64));
    let date_ok = (in_table & _mm512_cmpeq_epi64_mask(and!(srl!(g1, 16), byte), v!(b'T'))) | all_ff;
    let t = xor!(or!(srl!(g1, 24), sll!(g2, 40)), v!(0x3030_3A30_303A_3030u64));
    let over = and!(or!(add!(t, v!(0x767A_7F76_7A7F_767Du64)), t), v!(HIGH));
    let y = add!(add!(t, sll!(t, 9)), sll!(t, 11)); // t * 0xA01
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

    // F3 (see `super::digits16`): 1 to 16 digits. They are in `g0` and `g1`;
    // moved up by `16 - n` bytes they are the sixteen bytes that end where
    // the field ends, with zeros before the field's start (which, like
    // whatever is really there, are not looked at).
    let up = sll!(sub!(v!(16), n), 3);
    let hi8 = _mm512_sllv_epi64(g0, up);
    let lo8 = or!(
        _mm512_sllv_epi64(g1, up),
        or!(_mm512_srlv_epi64(g0, sub!(v!(64), up)), _mm512_sllv_epi64(g0, sub!(up, v!(64))))
    );
    let nlo = _mm512_min_epu64(n, v!(8));
    let keep_lo = _mm512_sllv_epi64(ones, sub!(v!(64), sll!(nlo, 3)));
    let keep_hi = _mm512_sllv_epi64(ones, sub!(v!(64), sll!(sub!(n, nlo), 3)));
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

    // Which form it is, and what that makes of the field.
    let kf12 = kf1 | kf2;
    let maybe_digits = _mm512_cmpneq_epi64_mask(n, zero)
        & _mm512_cmplt_epu64_mask(sub!(and!(g0, byte), v!(b'0')), v!(10))
        & _mm512_cmplt_epu64_mask(sub!(srl!(gx, 56), v!(b'0')), v!(10));
    let dead = (kf12 & bad1) | (!(kf12 | kf3) & maybe_digits);
    let ok = (kf1 & !bad1 & _mm512_cmpeq_epi64_mask(sfx, v!(b'Z')))
        | (kf2 & !bad1 & ok2)
        | (kf3 & _mm512_testn_epi64_mask(not_digit, not_digit));
    (dead, ok, _mm512_mask_mov_epi64(_mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(kf3, ms3), kf1, ms1), kf2, ms2))
}

/// The numbers of the lines `i..i + 8` that have a bit in `which`, in order,
/// one to a byte from the lowest (the rest of the word is zero).
#[inline(always)]
unsafe fn picked(i: usize, which: u64) -> u64 {
    _pext_u64(0x0706_0504_0302_0100u64 + LOW * i as u64, _pdep_u64(which, LOW) * 0xFF)
}

// ------------------------------------------------------------------ events

/// `super::id_trace_at` for eight event IDs of `n` bytes at `a`; their first
/// bytes and the eight bytes after them (`w1`, as they are in the input) are
/// given. Returns the partitions (one per lane, with the four more bits of
/// the hash), the first trace words without the positions, and the lanes
/// whose ID has neither the regular shape (at most 9 bytes) nor 33 bytes:
/// those are for the scalar path.
///
/// Both hashes of ids.rs are computed and one is chosen, as there: one ID in
/// eight is long, so most groups of eight lines have one.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn id_trace8(idp: &IdParams, a: __m512i, first: __m512i, w1: __m512i, n: __m512i) -> (__m512i, __m512i, __mmask8) {
    let long = _mm512_cmpeq_epi64_mask(n, v!(LONG_LEN));
    // The first multiplication is the same in both: of the first byte's
    // product with the word after it, which for `ids::hash9` is the rest of
    // the ID zero-padded and for `ids::hash33` the word as it is.
    let keep = _mm512_srlv_epi64(v!(u64::MAX), sub!(v!(72), sll!(n, 3)));
    let rest = and!(w1, keep);
    let x = xor!(_mm512_mask_mov_epi64(rest, long, w1), _mm512_mullo_epi64(first, v!(0xD6E8_FEB8_6659_FD93u64)));
    let m = _mm512_mullo_epi64(x, v!(0x9E37_79B9_7F4A_7C15u64));
    let h9 = xor!(m, srl!(m, 32));
    // ids::hash33: the three more words of an ID of 33 bytes (only those
    // lanes' memory is read), then `ids::finish`.
    let b = _mm512_mullo_epi64(gather_if!(long, a, 9), v!(0xC2B2_AE3D_27D4_EB4Fu64));
    let c = _mm512_mullo_epi64(gather_if!(long, a, 17), v!(0x1656_67B1_9E37_79F9u64));
    let d = _mm512_mullo_epi64(gather_if!(long, a, 25), v!(0xFF51_AFD7_ED55_8CCDu64));
    let s = xor!(xor!(m, _mm512_rol_epi64::<32>(b)), xor!(_mm512_rol_epi64::<17>(c), _mm512_rol_epi64::<47>(d)));
    let s = xor!(s, srl!(s, 29));
    let s = _mm512_mullo_epi64(s, v!(0xBF58_476D_1CE4_E5B9u64));
    let h = _mm512_mask_mov_epi64(h9, long, xor!(s, srl!(s, 32)));
    // `super::trace_from`.
    let pk = or!(srl!(mul!(srl!(h, 32), v!(idp.pd)), 32), sll!(and!(srl!(h, 40), v!(15)), 16));
    let trace = _mm512_sll_epi64(and!(h, v!(idp.fp_mask)), _mm_cvtsi32_si128(idp.loc_bits as i32));
    // A NUL among a short ID's bytes (its first, or one of the rest: `keep`
    // has those, and is empty for an ID that is too long anyway): zero-padded
    // it would hash as a shorter ID does, so the scalar path takes it.
    let nul = _mm512_test_epi64_mask(_mm512_andnot_si512(w1, sub!(w1, v!(LOW))), and!(keep, v!(HIGH)))
        | _mm512_cmpeq_epi64_mask(first, _mm512_setzero_si512());
    (pk, trace, !long & (_mm512_cmpgt_epu64_mask(n, v!(9)) | nul))
}

/// Decode the batch's event lines: `super::stage_decode`, on this path.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
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

#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn decode8(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    let dt = &ctx.dates;
    let idp = IdParams::new(ctx);
    let table_days = days(dt);
    let zero = _mm512_setzero_si512();
    let (ws, t0, span) = (v!(ctx.ws), v!(ctx.t0), v!((ctx.we - ctx.ws) as u64));
    let first_day = v!(dt.first_day_ms);
    // A position in the window's days, capped at the start of the last one
    // (see the division below).
    let day_cap = v!((dt.day_text.len() as u64 - 1) * DAY_MS as u64);
    let mut nmore = 0usize;
    let mut i = 0usize;
    while i < n {
        let live: u8 = if n - i >= W { 0xFF } else { (1u8 << (n - i)) - 1 };
        let p = lines!(st.p, i, live);
        let (start, len) = fields8(lines!(st.sep, i, live));

        // event_id: its first byte, and the rest (up to 8 bytes) zero-padded.
        let (a0, n0) = field!(p, start, len, 0);
        let first = srl!(gather!(a0, -7), 56);
        let (pk, trace, dead_id) = id_trace8(&idp, a0, first, gather!(a0, 1), n0);
        _mm256_storeu_si256(st.pk.as_mut_ptr().add(i) as *mut __m256i, _mm512_cvtepi64_epi32(pk));
        _mm512_storeu_si512(st.trace.as_mut_ptr().add(i) as *mut _, or!(trace, add!(v!(loc0), p)));

        // user_id: one that is its own key, or none (key 0). One of 33 bytes
        // is taken for a long ID here, and for good: `super::stage_long`
        // looks at it and finds its key (0 until then).
        let (a1, n1) = field!(p, start, len, 1);
        let (user_odd, key) = key8(a1, n1, b'u');
        let user_long = _mm512_cmpeq_epi64_mask(n1, v!(LONG_LEN));
        let user_bad = user_odd & !user_long;
        _mm512_storeu_si512(st.key.as_mut_ptr().add(i) as *mut _, _mm512_maskz_mov_epi64(!user_odd, key));
        for j in 0..W {
            *st.home.get_unchecked_mut(i + j) = ctx.table.prefetch_home(*st.key.get_unchecked(i + j)) as u32;
        }

        let (a2, n2) = field!(p, start, len, 2);
        let (ts_dead, ts_ok, ms) = ts8(&table_days, a2, n2);
        let in_window = _mm512_cmplt_epu64_mask(sub!(ms, ws), span);
        _mm256_storeu_si256(st.ts.as_mut_ptr().add(i) as *mut __m256i, _mm512_cvtepi64_epi32(sub!(ms, t0)));

        // ingest_ts on the same UTC day as ts? `super::stage_decode` compares
        // its first ten bytes with the text of the day of ts; for an event
        // that is valid (the flag is not used otherwise), that is: its date
        // is one of the table's days, the one `ms` falls on. The window's day
        // of `ms` is `(ms - first day) / DAY_MS`; capped, the dividend is
        // below 2^32, and for such `x`, `x / DAY_MS` is `(x >> 10) / 84375`,
        // which is `((x >> 10) * 3335999724) >> 48` exactly.
        let (a3, n3) = field!(p, start, len, 3);
        let (i0, i8, i12) = (gather!(a3, 0), gather!(a3, 8), gather!(a3, 12));
        let x = _mm512_min_epu64(sub!(ms, first_day), day_cap);
        let day = srl!(mul!(srl!(x, 10), v!(3_335_999_724u64)), 48);
        let (ingest_in_table, ingest_day_ms) = day_of(&table_days, i0, i8);
        let same_day = ingest_in_table
            & _mm512_cmpeq_epi64_mask(ingest_day_ms, add!(first_day, mul!(day, v!(DAY_MS))))
            & _mm512_cmpeq_epi64_mask(srl!(i12, 56), v!(b'Z'));

        // view / cart / purchase, exactly; a product.
        let (a4, n4) = field!(p, start, len, 4);
        let w = gather!(a4, 0);
        let wl = and!(w, v!(0xFFFF_FFFFu64));
        let k_purchase = _mm512_cmpeq_epi64_mask(n4, v!(8)) & _mm512_cmpeq_epi64_mask(w, v!(PURCHASE));
        let k_view = _mm512_cmpeq_epi64_mask(n4, v!(4))
            & (_mm512_cmpeq_epi64_mask(wl, v!(VIEW)) | _mm512_cmpeq_epi64_mask(wl, v!(CART)));
        let has_product = _mm512_test_epi64_mask(len, v!(0xFF00_0000_0000u64));

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
        let fl = or!(fl, _mm512_maskz_mov_epi64(valid & user_long, v!(F_LONG)));
        (st.flags.as_mut_ptr().add(i) as *mut u64).write_unaligned(_mm_cvtsi128_si64(_mm512_cvtepi64_epi8(fl)) as u64);
        // The lines with more to do, in order.
        let mk = ((ingest | purchase) & live) as u64;
        (st.more.as_mut_ptr().add(nmore) as *mut u64).write_unaligned(picked(i, mk));
        nmore += mk.count_ones() as usize;
        i += W;
    }
    nmore
}

/// SS_SIMD_CHECK=1: the batch through both paths. The split of every line
/// again by the ordinary splitter, then both decoders; any difference that the
/// later stages could see stops the run with the line.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
unsafe fn checked(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    let (lines, seps) = (st.p, st.sep);
    let fail = |i: usize, what: &str, a: u64, b: u64| -> ! {
        let line = std::slice::from_raw_parts(lines[i], (seps[i] >> 48 & 0xFF) as usize);
        eprintln!(
            "SS_SIMD_CHECK: {what} differs on event line {i} of a batch of {n}: wide {a:#x}, ordinary {b:#x}\n{}",
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

// ------------------------------------------------------------------ clicks

/// Decode the batch's click lines: `super::click_decode`, on this path.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
pub(super) unsafe fn click_decode(ctx: &Ctx, st: &mut ClickStage, n: usize) {
    if !ctx.dates.wide_ok {
        return super::click_decode(ctx, st, n);
    }
    if wide_check() {
        return click_checked(ctx, st, n);
    }
    click_decode8(ctx, st, n)
}

#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn click_decode8(ctx: &Ctx, st: &mut ClickStage, n: usize) {
    let table_days = days(&ctx.dates);
    let direct = ctx.campaigns().index.direct();
    let (campaigns, ncampaigns) = (direct.as_ptr() as *const i32, v!(direct.len()));
    let (t0, span) = (v!(ctx.t0), v!((ctx.we - ctx.t0) as u64));
    let low32 = v!(0xFFFF_FFFFu64);
    let byte = v!(0xFF);
    let mut i = 0usize;
    while i < n {
        let live: u8 = if n - i >= W { 0xFF } else { (1u8 << (n - i)) - 1 };
        let p = lines!(st.p, i, live);
        let (start, len) = fields8(lines!(st.sep, i, live));

        // click_id: one of the regular shape is its own key; any other is
        // keyed by where it is (ids::ref_word).
        let (a0, n0) = field!(p, start, len, 0);
        let (click_odd, click) = key8(a0, n0, b'k');
        let by_ref = or!(or!(v!(crate::ids::IRREGULAR | crate::ids::REF), sll!(n0, 47)), a0);
        _mm512_storeu_si512(st.click.as_mut_ptr().add(i) as *mut _, _mm512_mask_mov_epi64(click, click_odd, by_ref));
        // user_id: its own key likewise; one of 33 bytes is taken for a long
        // ID, for `super::click_long` to look at (its key here is 0).
        let (a1, n1) = field!(p, start, len, 1);
        let (user_odd, user) = key8(a1, n1, b'u');
        let user_long = _mm512_cmpeq_epi64_mask(n1, v!(LONG_LEN));
        let user_bad = user_odd & !user_long;
        _mm512_storeu_si512(st.user.as_mut_ptr().add(i) as *mut _, user);
        for j in 0..W {
            *st.home.get_unchecked_mut(i + j) = ctx.table.prefetch_home(*st.user.get_unchecked(i + j)) as u32;
        }

        // campaign_id: `c_` and 1 to 4 digits without a leading zero is a
        // number for the table (see `super::click_decode`: the same
        // arithmetic, in the low 32 bits of each lane).
        let (a2, n2) = field!(p, start, len, 2);
        let c = gather!(a2, 0);
        let m = sub!(n2, v!(2));
        let digits = and!(
            xor!(and!(srl!(c, 16), low32), v!(0x3030_3030u64)),
            _mm512_srlv_epi64(low32, sub!(v!(32), sll!(m, 3)))
        );
        let x = and!(sub!(digits, v!(1)), low32);
        let campaign_bad = _mm512_cmpgt_epu64_mask(sub!(m, v!(1)), v!(3))
            | _mm512_test_epi64_mask(or!(add!(x, v!(0x7676_7677u64)), x), v!(0x8080_8080u64))
            | _mm512_cmpneq_epi64_mask(and!(c, v!(0xFFFFu64)), v!(u16::from_le_bytes(*b"c_")));
        let t = and!(_mm512_sllv_epi64(digits, sub!(v!(32), sll!(m, 3))), low32);
        let pairs = and!(add!(add!(sll!(t, 3), sll!(t, 1)), srl!(t, 8)), low32);
        let number = add!(mul!(and!(pairs, byte), v!(100)), and!(srl!(pairs, 16), byte));
        let campaign = _mm512_cvtepu32_epi64(_mm512_mask_i64gather_epi32::<4>(
            _mm256_set1_epi32(ABSENT as i32),
            _mm512_cmplt_epu64_mask(number, ncampaigns),
            number,
            campaigns,
        ));

        let (a3, n3) = field!(p, start, len, 3);
        let (ts_dead, ts_ok, ms) = ts8(&table_days, a3, n3);
        let rel = sub!(ms, t0);
        let dead = user_bad | campaign_bad | ts_dead;
        let valid = ts_ok & _mm512_cmplt_epu64_mask(rel, span) & _mm512_cmpneq_epi64_mask(campaign, v!(ABSENT)) & !dead;
        _mm512_storeu_si512(st.tc.as_mut_ptr().add(i) as *mut _, or!(_mm512_slli_epi64::<CAMPAIGN_BITS>(rel), campaign));
        let fl = or!(
            or!(_mm512_maskz_mov_epi64(dead, v!(C_DEAD)), _mm512_maskz_mov_epi64(valid, v!(C_VALID))),
            _mm512_maskz_mov_epi64(valid & user_long, v!(C_LONG))
        );
        (st.flags.as_mut_ptr().add(i) as *mut u64).write_unaligned(_mm_cvtsi128_si64(_mm512_cvtepi64_epi8(fl)) as u64);
        i += W;
    }
}

/// SS_SIMD_CHECK=1 for clicks: see `checked`.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
unsafe fn click_checked(ctx: &Ctx, st: &mut ClickStage, n: usize) {
    let (lines, seps) = (st.p, st.sep);
    let fail = |i: usize, what: &str, a: u64, b: u64| -> ! {
        let line = std::slice::from_raw_parts(lines[i], (seps[i] >> 24 & 0xFF) as usize);
        eprintln!(
            "SS_SIMD_CHECK: {what} differs on click line {i} of a batch of {n}: wide {a:#x}, ordinary {b:#x}\n{}",
            String::from_utf8_lossy(line)
        );
        std::process::abort()
    };
    let mut one = *st;
    for i in 0..n {
        let (used, added) = split_click(&mut one, 0, st.p[i]);
        if !added || used != (st.sep[i] >> 24 & 0xFF) as usize + 1 {
            fail(i, "split", st.sep[i], used as u64);
        }
        if one.sep[0] != st.sep[i] {
            fail(i, "separators", st.sep[i], one.sep[0]);
        }
    }
    let mut base = *st;
    super::click_decode(ctx, &mut base, n);
    click_decode8(ctx, st, n);
    for i in 0..n {
        let (fw, fb) = (st.flags[i], base.flags[i]);
        if fw != fb {
            fail(i, "flags", fw as u64, fb as u64);
        }
        if fw & C_VALID != 0 {
            if st.user[i] != base.user[i] {
                fail(i, "user key", st.user[i], base.user[i]);
            }
            if st.click[i] != base.click[i] {
                fail(i, "click key", st.click[i], base.click[i]);
            }
            if st.tc[i] != base.tc[i] {
                fail(i, "time and campaign", st.tc[i], base.tc[i]);
            }
            if st.home[i] != base.home[i] {
                fail(i, "home slot", st.home[i] as u64, base.home[i] as u64);
            }
        }
    }
}

// ------------------------------------------------------------------ emit

/// The people of eight users, by their keys and home slots: `Table::person_at`
/// for the lanes of `want`. The four keys of each home line are fetched and
/// compared; a key that is there names its slot, and the slot's parent, if it
/// has one, the person. Returns the people, and the lanes whose key is not in
/// its home line: a user seen for the first time, or a line that was full.
/// Those are for `Table::person_at` itself, which knows how to insert.
/// (`slots` is the table: 16 bytes a slot, the key and then the parent.)
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
#[inline]
unsafe fn people8(slots: *const u8, want: __mmask8, home: __m512i, key: __m512i) -> (__m512i, __mmask8) {
    let zero = _mm512_setzero_si512();
    let at = sll!(home, 4);
    let m0 = _mm512_mask_cmpeq_epi64_mask(want, _mm512_mask_i64gather_epi64::<1>(zero, want, at, slots as *const i64), key);
    let m1 =
        _mm512_mask_cmpeq_epi64_mask(want, _mm512_mask_i64gather_epi64::<1>(zero, want, at, slots.add(16) as *const i64), key);
    let m2 =
        _mm512_mask_cmpeq_epi64_mask(want, _mm512_mask_i64gather_epi64::<1>(zero, want, at, slots.add(32) as *const i64), key);
    let m3 =
        _mm512_mask_cmpeq_epi64_mask(want, _mm512_mask_i64gather_epi64::<1>(zero, want, at, slots.add(48) as *const i64), key);
    let found = m0 | m1 | m2 | m3;
    // The first slot of the line that holds the key (a key is in one slot only).
    let which = _mm512_maskz_mov_epi64(
        !m0,
        _mm512_mask_mov_epi64(_mm512_mask_mov_epi64(_mm512_maskz_mov_epi64(m3, v!(3)), m2, v!(2)), m1, v!(1)),
    );
    let slot = add!(home, which);
    let parent = and!(
        _mm512_mask_i64gather_epi64::<1>(zero, found, sll!(slot, 4), slots.add(8) as *const i64),
        v!(0xFFFF_FFFFu64)
    );
    (_mm512_mask_mov_epi64(sub!(parent, v!(1)), _mm512_cmpeq_epi64_mask(parent, zero), slot), want & !found)
}

/// `people8` for the lines `i..i + 8` of a batch, the lanes it left to
/// `Table::person_at` filled in by it.
macro_rules! people_of {
    ($ctx:expr, $want:expr, $homes:expr, $keys:expr, $i:expr) => {{
        let home = _mm512_cvtepu32_epi64(_mm256_loadu_si256($homes.as_ptr().add($i) as *const __m256i));
        let key = _mm512_loadu_si512($keys.as_ptr().add($i) as *const _);
        let (mut people, mut missed) = people8($ctx.table.slots(), $want, home, key);
        if missed != 0 {
            let mut one = [0u64; W];
            _mm512_storeu_si512(one.as_mut_ptr() as *mut _, people);
            while missed != 0 {
                let j = missed.trailing_zeros() as usize;
                one[j] = $ctx.table.person_at($homes[$i + j] as usize, $keys[$i + j]) as u64;
                missed &= missed - 1;
            }
            people = _mm512_loadu_si512(one.as_ptr() as *const _);
        }
        people
    }};
}

/// SS_WEMIT=0: the ordinary emit stages on this path too.
fn wide_emit() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("SS_WEMIT").is_ok_and(|v| v == "0"))
}

/// What goes to a partition of a writer: (partition, word, word).
type Pushed = (usize, u64, u64);

/// What `super::stage_emit` writes for line `i`, which is not for the scalar
/// path: the purchase, if it is one, and the trace.
unsafe fn emit_reference(ctx: &Ctx, st: &Stage, i: usize) -> (Option<Pushed>, Pushed) {
    let fl = st.flags[i];
    let pk = st.pk[i] as u64;
    let (mut w2, mut purchase) = (0, None);
    if fl & F_VALID != 0 {
        let person = ctx.table.person_at(st.home[i] as usize, st.key[i]) as u64;
        let ts = st.ts[i] as u64;
        w2 = trace_valid(pk >> 16, person, ts);
        if fl & F_PURCHASE != 0 {
            purchase = Some(((person >> LOCAL_BITS) as usize, (person & ((1 << LOCAL_BITS) - 1)) << 31 | ts, st.extra[i]));
            w2 |= T_PURCHASE;
        }
    }
    (purchase, (pk as usize & 0xFFFF, st.trace[i], w2))
}

/// `super::stage_emit` on this path: the people and the traces' second words
/// of eight lines at a time, then the pushes, a line at a time as there.
/// (`SS_SIMD_CHECK=1`: what is pushed must be what `stage_emit` would push.)
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
pub(super) unsafe fn emit(ctx: &Ctx, out: &mut EventOut, st: &Stage, n: usize) {
    if !wide_emit() {
        return stage_emit(ctx, out, st, n);
    }
    let mut second = [0u64; NB];
    let mut people = [0u64; NB];
    let mut i = 0usize;
    while i < n {
        let live: u8 = if n - i >= W { 0xFF } else { (1u8 << (n - i)) - 1 };
        let fl = _mm512_cvtepu8_epi64(_mm_loadl_epi64(st.flags.as_ptr().add(i) as *const __m128i));
        // Valid, and not for the scalar path.
        let valid = _mm512_mask_testn_epi64_mask(_mm512_test_epi64_mask(fl, v!(F_VALID)) & live, fl, v!(F_DEAD));
        let person = people_of!(ctx, valid, st.home, st.key, i);
        let ts = _mm512_cvtepu32_epi64(_mm256_loadu_si256(st.ts.as_ptr().add(i) as *const __m256i));
        let pk = _mm512_cvtepu32_epi64(_mm256_loadu_si256(st.pk.as_ptr().add(i) as *const __m256i));
        // `trace_valid`, and `T_PURCHASE` for a purchase.
        let w2 = or!(or!(v!(T_VALID), sll!(and!(srl!(pk, 16), v!(15)), 58)), or!(sll!(person, 31), ts));
        let w2 = or!(
            _mm512_maskz_mov_epi64(valid, w2),
            _mm512_maskz_mov_epi64(valid & _mm512_test_epi64_mask(fl, v!(F_PURCHASE)), v!(T_PURCHASE))
        );
        _mm512_storeu_si512(second.as_mut_ptr().add(i) as *mut _, w2);
        _mm512_storeu_si512(people.as_mut_ptr().add(i) as *mut _, person);
        i += W;
    }
    let check = wide_check();
    let (kw, pw) = (out.k.cursors(), out.ps.cursors());
    let pool = &ctx.pool;
    for i in 0..n {
        let fl = *st.flags.get_unchecked(i);
        if fl & F_DEAD != 0 {
            continue;
        }
        let purchase: Option<Pushed> = if fl & (F_VALID | F_PURCHASE) == F_VALID | F_PURCHASE {
            let person = *people.get_unchecked(i);
            let rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | *st.ts.get_unchecked(i) as u64;
            Some(((person >> LOCAL_BITS) as usize, rec, *st.extra.get_unchecked(i)))
        } else {
            None
        };
        let trace: Pushed = (*st.pk.get_unchecked(i) as usize & 0xFFFF, *st.trace.get_unchecked(i), *second.get_unchecked(i));
        if check {
            let want = emit_reference(ctx, st, i);
            if want != (purchase, trace) {
                eprintln!(
                    "SS_SIMD_CHECK: emit differs on event line {i} of a batch of {n}: wide {:x?}, ordinary {want:x?}\n{}",
                    (purchase, trace),
                    String::from_utf8_lossy(std::slice::from_raw_parts(st.p[i], (st.sep[i] >> 48 & 0xFF) as usize))
                );
                std::process::abort();
            }
        }
        if let Some((part, rec, extra)) = purchase {
            out.ps.push2_at(pw, pool, part, rec, extra);
        }
        out.k.push2_at(kw, pool, trace.0, trace.1, trace.2);
    }
}

/// `super::click_emit` on this path.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq")]
pub(super) unsafe fn click_emit(ctx: &Ctx, c: &mut PartWriter, st: &ClickStage, n: usize) {
    if !wide_emit() {
        return super::click_emit(ctx, c, st, n);
    }
    let mut people = [0u64; NB];
    let mut i = 0usize;
    while i < n {
        let live: u8 = if n - i >= W { 0xFF } else { (1u8 << (n - i)) - 1 };
        let fl = _mm512_cvtepu8_epi64(_mm_loadl_epi64(st.flags.as_ptr().add(i) as *const __m128i));
        let valid = _mm512_cmpeq_epi64_mask(fl, v!(C_VALID)) & live;
        let person = people_of!(ctx, valid, st.home, st.user, i);
        _mm512_storeu_si512(people.as_mut_ptr().add(i) as *mut _, person);
        i += W;
    }
    let check = wide_check();
    let cw = c.cursors();
    let pool = &ctx.pool;
    for i in 0..n {
        if *st.flags.get_unchecked(i) != C_VALID {
            continue;
        }
        let (part, w0) = click_words(*people.get_unchecked(i), *st.tc.get_unchecked(i));
        if check {
            // What `super::click_emit` pushes.
            let want = click_words(ctx.table.person_at(st.home[i] as usize, st.user[i]) as u64, st.tc[i]);
            if want != (part, w0) {
                eprintln!(
                    "SS_SIMD_CHECK: emit differs on click line {i} of a batch of {n}: wide {:x?}, ordinary {want:x?}\n{}",
                    (part, w0),
                    String::from_utf8_lossy(std::slice::from_raw_parts(st.p[i], (st.sep[i] >> 24 & 0xFF) as usize))
                );
                std::process::abort();
            }
        }
        c.push2_at(cw, pool, part, w0, *st.click.get_unchecked(i));
    }
}
