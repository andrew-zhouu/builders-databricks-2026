//! The decode stages on processors with AVX-512 (`super::wide_kind`): the same
//! results as `super::stage_decode` and `super::click_decode`, eight lines at
//! a time. Nothing here needs more than AVX-512 F, BW, VL and DQ, so both wide
//! parser instances use this code: the one for processors with VBMI2 and the
//! one for those without, which has its own copy of the decode functions
//! (wide_decode.rs, see `half`) for the sake of its clock.
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

/// The numbers of the lines `i..i + 8` that have a bit in `which`, in order,
/// one to a byte from the lowest (the rest of the word is zero).
#[inline(always)]
unsafe fn picked(i: usize, which: u64) -> u64 {
    _pext_u64(0x0706_0504_0302_0100u64 + LOW * i as u64, _pdep_u64(which, LOW) * 0xFF)
}

// ------------------------------------------------------------- multiplying
//
// The decode functions (wide_decode.rs) exist twice. Here, for the instance
// for processors with VBMI2, every multiplication is one 512-bit instruction.
// In `half`, for the instance for Skylake-X and Cascade Lake, it is two
// 256-bit ones: those processors run 512-bit multiplications (and only those,
// of what this parser uses) at their lowest clock, and everything else here
// at the level 256-bit multiplications need anyway.

/// The product of the low 32 bits of each lane (both below 2^32 wherever this
/// is used, so it is the product).
macro_rules! mul {
    ($a:expr, $b:expr) => {
        _mm512_mul_epu32($a, $b)
    };
}
/// The low 64 bits of the product of each lane.
macro_rules! mullo {
    ($a:expr, $b:expr) => {
        _mm512_mullo_epi64($a, $b)
    };
}
macro_rules! maddubs {
    ($a:expr, $b:expr) => {
        _mm512_maddubs_epi16($a, $b)
    };
}
macro_rules! madd {
    ($a:expr, $b:expr) => {
        _mm512_madd_epi16($a, $b)
    };
}
/// `$t * 0xA01` in each lane: two decimal digits, a byte apart, to their value.
macro_rules! times_a01 {
    ($t:expr) => {
        add!(add!($t, sll!($t, 9)), sll!($t, 11))
    };
}
/// `$t * 10` in each lane.
macro_rules! times10 {
    ($t:expr) => {
        add!(sll!($t, 3), sll!($t, 1))
    };
}

include!("wide_decode.rs");

/// The decode functions with every multiplication on 256-bit halves, and no
/// 512-bit multiplication at all (scripts/remote/mulcount.sh checks that in
/// the binary). The instructions are written out as such: the compiler joins
/// two 256-bit multiplications of the halves of one register into the 512-bit
/// one, and makes multiplications of sums of shifts.
pub(super) mod half {
    use super::*;

    /// One instruction, given by name, on the two halves of its operands.
    macro_rules! halves {
        ($insn:literal, $a:expr, $b:expr) => {{
            let (a, b): (__m512i, __m512i) = ($a, $b);
            let (lo, hi): (__m256i, __m256i);
            core::arch::asm!(
                concat!($insn, " {lo}, {alo}, {blo}"),
                concat!($insn, " {hi}, {ahi}, {bhi}"),
                alo = in(ymm_reg) _mm512_castsi512_si256(a),
                blo = in(ymm_reg) _mm512_castsi512_si256(b),
                ahi = in(ymm_reg) _mm512_extracti64x4_epi64::<1>(a),
                bhi = in(ymm_reg) _mm512_extracti64x4_epi64::<1>(b),
                lo = out(ymm_reg) lo,
                hi = out(ymm_reg) hi,
                options(pure, nomem, nostack, preserves_flags)
            );
            _mm512_inserti64x4::<1>(_mm512_castsi256_si512(lo), hi)
        }};
    }
    macro_rules! mul {
        ($a:expr, $b:expr) => {
            halves!("vpmuludq", $a, $b)
        };
    }
    macro_rules! mullo {
        ($a:expr, $b:expr) => {
            halves!("vpmullq", $a, $b)
        };
    }
    macro_rules! maddubs {
        ($a:expr, $b:expr) => {
            halves!("vpmaddubsw", $a, $b)
        };
    }
    macro_rules! madd {
        ($a:expr, $b:expr) => {
            halves!("vpmaddwd", $a, $b)
        };
    }
    /// As shifts and additions, which are not multiplications to the clock.
    macro_rules! times_a01 {
        ($t:expr) => {{
            let t: __m512i = $t;
            let y: __m512i;
            core::arch::asm!(
                "vpsllq {y}, {t}, 9",
                "vpsllq {u}, {t}, 11",
                "vpaddq {y}, {y}, {t}",
                "vpaddq {y}, {y}, {u}",
                t = in(zmm_reg) t,
                y = out(zmm_reg) y,
                u = out(zmm_reg) _,
                options(pure, nomem, nostack, preserves_flags)
            );
            y
        }};
    }
    macro_rules! times10 {
        ($t:expr) => {{
            let t: __m512i = $t;
            let y: __m512i;
            core::arch::asm!(
                "vpsllq {y}, {t}, 3",
                "vpsllq {u}, {t}, 1",
                "vpaddq {y}, {y}, {u}",
                t = in(zmm_reg) t,
                y = out(zmm_reg) y,
                u = out(zmm_reg) _,
                options(pure, nomem, nostack, preserves_flags)
            );
            y
        }};
    }

    include!("wide_decode.rs");
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
