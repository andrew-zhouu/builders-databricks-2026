//! Exact integer keys for user and click IDs, and fingerprints for event IDs.
//!
//! **User and click IDs.** A key is a 64-bit word that says exactly which ID it
//! stands for: equal keys mean equal IDs, and (for users) equal IDs have equal
//! keys. There are four kinds:
//!
//! * *Regular*: the usual prefix letter (`u` or `k`) followed by 1 to 8 bytes
//!   that are all ASCII and not NUL. The key is those bytes as a big-endian
//!   integer, zero-padded on the right: injective (a shorter ID never equals a
//!   longer one because no byte is zero), never 0, top bit clear, and ordered
//!   exactly like the IDs byte-wise.
//! * *Long* (users): the prefix letter followed by exactly 32 lowercase
//!   hexadecimal digits, which one ID in eight is. Those 128 bits are kept in a
//!   lock-free table (`LongIds`), and the key is the ID's slot there.
//! * *Reference* (clicks): a click ID is never looked up, only compared with
//!   another on a tie, so any click ID that is not regular is keyed by where
//!   its bytes are in the mapped input (the mappings stay for the whole run).
//!   Two copies of a click then have different keys, which is harmless: the
//!   keys are only ever compared through `less`, byte-wise.
//! * Everything else (other first letter, upper case, non-ASCII, ...) is
//!   interned in a dictionary behind a lock and keyed by its index. Rare.
//!
//! **Event IDs** are only ever compared for equality, and candidates for
//! equality are confirmed by reading the IDs again from the input, so an event
//! ID needs no exact key: `event_hash` is an ordinary 64-bit hash of its bytes.

use core::arch::x86_64::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

use crate::sys::map_anon;

/// Not a regular key.
pub const IRREGULAR: u64 = 1 << 63;
/// With `IRREGULAR`: a slot of the table of long IDs (the low 32 bits).
pub const LONG: u64 = 1 << 62;
/// With `IRREGULAR`: a reference to the ID's bytes, `len << 47 | address`.
pub const REF: u64 = 1 << 61;
const REF_ADDR: u64 = (1 << 47) - 1;
/// The longest ID a reference can describe.
const REF_MAX: usize = (1 << 14) - 1;
/// Bytes in a long ID: the prefix letter and 32 hexadecimal digits.
pub const LONG_LEN: usize = 33;
const HIGH_BITS: u64 = 0x8080_8080_8080_8080;
const LOW_BITS: u64 = 0x0101_0101_0101_0101;

/// The key of a regular ID with prefix `prefix`, or `None`.
#[inline]
pub fn raw_key(b: &[u8], prefix: u8) -> Option<u64> {
    let n = b.len();
    if !(2..=9).contains(&n) || b[0] != prefix {
        return None;
    }
    let mut w = [0u8; 8];
    w[..n - 1].copy_from_slice(&b[1..]);
    if b[1..].iter().any(|&c| c == 0 || c >= 0x80) {
        return None;
    }
    Some(u64::from_be_bytes(w))
}

/// As `raw_key`, for `n` bytes at `a` with at least 9 readable bytes.
/// Returns (regular, key); the key is meaningless when not regular.
#[inline(always)]
pub unsafe fn raw_key_fast(a: *const u8, n: usize, prefix: u8) -> (bool, u64) {
    // The payload is bytes 1..n: the low `bits` bits of `raw`.
    let bits = 8u32.wrapping_mul((n as u32).wrapping_sub(1));
    let raw = (a.add(1) as *const u64).read_unaligned();
    // Among the payload's bytes: a NUL (the first test marks a zero byte, and
    // nothing below the lowest one), or a byte that is not ASCII?
    let odd = ((raw.wrapping_sub(LOW_BITS) & !raw) | raw) & _bzhi_u64(HIGH_BITS, bits);
    let ok = (n.wrapping_sub(2) < 8) & (*a == prefix) & (odd == 0);
    (ok, _bzhi_u64(raw, bits).swap_bytes())
}

/// The length of a regular ID's payload (1 to 8) from its key.
#[inline(always)]
pub fn raw_len(key: u64) -> usize {
    8 - (key.trailing_zeros() / 8) as usize
}

/// The value of the 32 lowercase hexadecimal digits at `a` (32 readable
/// bytes), most significant first, as (all of them are such digits, high 64
/// bits, low 64 bits). Upper-case digits do not count: `uAB..` and `uab..`
/// are two IDs.
#[inline(always)]
pub unsafe fn hex32(a: *const u8) -> (bool, u64, u64) {
    // (Sixteen bytes at a time: multiplies stay in 128-bit registers, see parse.rs.)
    #[inline(always)]
    unsafe fn half(a: *const u8) -> (bool, u64) {
        let v = _mm_loadu_si128(a as *const __m128i);
        let d = _mm_sub_epi8(v, _mm_set1_epi8(b'0' as i8));
        let l = _mm_sub_epi8(v, _mm_set1_epi8(b'a' as i8));
        let is_d = _mm_cmpeq_epi8(_mm_min_epu8(d, _mm_set1_epi8(9)), d);
        let is_l = _mm_cmpeq_epi8(_mm_min_epu8(l, _mm_set1_epi8(5)), l);
        let value = _mm_blendv_epi8(_mm_add_epi8(l, _mm_set1_epi8(10)), d, is_d);
        // Two digits make a byte: the first is its high half.
        let bytes = _mm_maddubs_epi16(value, _mm_set1_epi16(0x0110));
        let packed = _mm_packus_epi16(bytes, bytes);
        (_mm_movemask_epi8(_mm_or_si128(is_d, is_l)) == 0xFFFF, (_mm_cvtsi128_si64(packed) as u64).swap_bytes())
    }
    let (ok_hi, hi) = half(a);
    let (ok_lo, lo) = half(a.add(16));
    (ok_hi & ok_lo, hi, lo)
}

/// The 128 bits of a long ID with prefix `prefix`, or `None` if `b` is not one.
#[inline]
pub fn long_value(b: &[u8], prefix: u8) -> Option<(u64, u64)> {
    if b.len() != LONG_LEN || b[0] != prefix {
        return None;
    }
    let (ok, hi, lo) = unsafe { hex32(b.as_ptr().add(1)) };
    ok.then_some((hi, lo))
}

/// Sixteen lowercase hexadecimal digits of `x`, most significant first.
#[inline]
fn hex16(x: u64) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = b"0123456789abcdef"[(x >> (60 - 4 * i)) as usize & 15];
    }
    out
}

/// A long ID's bytes: the prefix and the 32 digits of its value.
#[inline]
pub fn long_bytes(prefix: u8, hi: u64, lo: u64) -> [u8; LONG_LEN] {
    let mut out = [0u8; LONG_LEN];
    out[0] = prefix;
    out[1..17].copy_from_slice(&hex16(hi));
    out[17..].copy_from_slice(&hex16(lo));
    out
}

/// A 64-bit hash of an event ID. Not exact: equal hashes are confirmed by
/// comparing the IDs themselves.
#[inline]
pub fn event_hash(b: &[u8]) -> u64 {
    let n = b.len();
    if n <= 9 && !b.contains(&0) {
        // The usual shape, a letter and up to 8 more bytes. (None of them
        // NUL: padded with zeros, `x` and `x\0` would be one ID here, and
        // equal hashes of the same person at the same time are taken for
        // copies without a look at the IDs. Such an ID goes on below, where
        // its length is hashed with it.)
        let mut w = [0u8; 8];
        if n > 1 {
            w[..n - 1].copy_from_slice(&b[1..]);
        }
        return hash9(if n > 0 { b[0] } else { 0 }, u64::from_le_bytes(w));
    }
    if n == LONG_LEN {
        // The other usual shape: a letter and 32 more bytes.
        let w = |k: usize| u64::from_le_bytes(b[1 + 8 * k..9 + 8 * k].try_into().unwrap());
        return hash33(b[0], [w(0), w(1), w(2), w(3)]);
    }
    let mut buf = [0u8; 16];
    let head = n.min(16);
    buf[..head].copy_from_slice(&b[..head]);
    let lo = u64::from_le_bytes(buf[..8].try_into().unwrap());
    let hi = u64::from_le_bytes(buf[8..].try_into().unwrap());
    let mut h = mix16(lo, hi, n);
    // Longer IDs (never on the fast path): fold in the rest, 8 bytes at a time.
    for chunk in b[head..].chunks(8) {
        let mut w = [0u8; 8];
        w[..chunk.len()].copy_from_slice(chunk);
        h = finish((h ^ u64::from_le_bytes(w)).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    }
    h
}

/// The hash of an ID of at most 9 bytes: its first byte, and the bytes after
/// it little-endian and zero-padded.
#[inline(always)]
pub fn hash9(first: u8, rest: u64) -> u64 {
    mix9(rest ^ (first as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93))
}

/// The hash of an ID of exactly 33 bytes: its first byte, and the 32 bytes
/// after it as four little-endian words. Each word goes through a
/// multiplication of its own (a change in one word alone always changes the
/// sum), and `finish` spreads the sum over the partition bits at the top and
/// the fingerprint bits at the bottom. Only multiplications, xors, rotations
/// and shifts of 64-bit words: the wide path does the same on eight lanes.
#[inline(always)]
pub fn hash33(first: u8, w: [u64; 4]) -> u64 {
    let a = (w[0] ^ (first as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let b = w[1].wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    let c = w[2].wrapping_mul(0x1656_67B1_9E37_79F9);
    let d = w[3].wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    finish(a ^ b.rotate_left(32) ^ c.rotate_left(17) ^ d.rotate_left(47))
}

#[inline(always)]
fn mix9(x: u64) -> u64 {
    // One multiplication: its high half is well mixed, and folding that into
    // the low half is enough for a partition number (from the top) and a
    // fingerprint (from the bottom) that say different things.
    let a = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    a ^ (a >> 32)
}

/// The hash of an ID of `n <= 16` bytes whose bytes, zero-padded, are `lo` and `hi`.
#[inline(always)]
pub fn mix16(lo: u64, hi: u64, n: usize) -> u64 {
    finish((lo ^ hi.rotate_left(29) ^ (n as u64) << 56).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ hi)
}

#[inline(always)]
pub fn finish(mut h: u64) -> u64 {
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 32)
}

/// The long IDs of a run: a lock-free table of 128-bit values, each of which
/// keeps the slot it is first put in. A slot is two words, zero when empty;
/// a value with a zero half is not taken (the dictionary keeps such an ID).
/// A slot is claimed by its first word; its second follows at once, and a
/// reader that finds the first word it is looking for waits for the second.
pub struct LongIds {
    slots: *mut [AtomicU64; 2],
    /// Slots: a power of two.
    cap: usize,
    shift: u32,
    count: AtomicUsize,
    /// More IDs than the table was made for: the run's answer cannot be
    /// trusted to come in time, and the caller starts over without it.
    pub overflow: AtomicBool,
}
unsafe impl Send for LongIds {}
unsafe impl Sync for LongIds {}

impl LongIds {
    pub fn new(cap: usize) -> LongIds {
        let cap = cap.max(1 << 12).next_power_of_two();
        LongIds {
            slots: map_anon(cap * 16, true) as *mut [AtomicU64; 2],
            cap,
            shift: 64 - cap.trailing_zeros(),
            count: AtomicUsize::new(0),
            overflow: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    fn home(&self, hi: u64, lo: u64) -> usize {
        ((hi.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ lo).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) >> self.shift) as usize
    }

    /// Bring the home slot of a value into cache.
    #[inline(always)]
    pub fn prefetch(&self, hi: u64, lo: u64) {
        unsafe { _mm_prefetch::<_MM_HINT_T0>(self.slots.add(self.home(hi, lo)) as *const i8) };
    }

    /// The slot of the value, which is put in if it is new. `None`: a half of
    /// it is zero. Safe to call from many threads.
    #[inline]
    pub fn intern(&self, hi: u64, lo: u64) -> Option<u32> {
        if (hi == 0) | (lo == 0) {
            return None;
        }
        let mut i = self.home(hi, lo);
        let mut probes = 0usize;
        loop {
            let slot = unsafe { &*self.slots.add(i) };
            let mut first = slot[0].load(Relaxed);
            if first == 0 {
                match slot[0].compare_exchange(0, hi, Relaxed, Relaxed) {
                    Ok(_) => {
                        slot[1].store(lo, Relaxed);
                        // Half full is as far as this goes (see `overflow`).
                        if self.count.fetch_add(1, Relaxed) >= self.cap / 2 {
                            self.overflow.store(true, Relaxed);
                        }
                        return Some(i as u32);
                    }
                    Err(actual) => first = actual,
                }
            }
            if first == hi {
                // Its second word is there, or about to be.
                let mut second = slot[1].load(Relaxed);
                while second == 0 {
                    std::hint::spin_loop();
                    second = slot[1].load(Relaxed);
                }
                if second == lo {
                    return Some(i as u32);
                }
            }
            i = (i + 1) & (self.cap - 1);
            probes += 1;
            if probes > self.cap {
                self.overflow.store(true, Relaxed);
                return Some(0);
            }
        }
    }

    /// The value in a slot.
    #[inline(always)]
    pub fn get(&self, slot: u32) -> (u64, u64) {
        let s = unsafe { &*self.slots.add(slot as usize & (self.cap - 1)) };
        (s[0].load(Relaxed), s[1].load(Relaxed))
    }

    pub fn len(&self) -> usize {
        self.count.load(Relaxed)
    }
}

/// Interns the IDs no other kind of key takes. Rare by construction, so one lock is fine.
pub struct Dict {
    inner: Mutex<(HashMap<Box<[u8]>, u32>, Vec<Box<[u8]>>)>,
}

impl Dict {
    pub fn new() -> Self {
        Dict { inner: Mutex::new((HashMap::new(), Vec::new())) }
    }

    pub fn intern(&self, b: &[u8]) -> u64 {
        let mut g = self.inner.lock().unwrap();
        if let Some(&i) = g.0.get(b) {
            return i as u64;
        }
        let i = g.1.len() as u32;
        g.1.push(b.into());
        g.0.insert(b.into(), i);
        i as u64
    }

    pub fn get(&self, idx: u64) -> Box<[u8]> {
        self.inner.lock().unwrap().1[idx as usize].clone()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().1.len()
    }
}

/// A reference key for the `n` bytes at `a` (see the module comment), or
/// `None` if they are too many for one.
#[inline(always)]
pub fn ref_key(a: *const u8, n: usize) -> Option<u64> {
    ((n <= REF_MAX) & (a as u64 <= REF_ADDR)).then_some(ref_word(a, n as u64))
}

/// `ref_key` for an ID known to be short enough (the fast paths: a field of a
/// line of at most 255 bytes).
#[inline(always)]
pub fn ref_word(a: *const u8, n: u64) -> u64 {
    IRREGULAR | REF | n << 47 | a as u64
}

/// Keys for one kind of ID (users: prefix `u`, with long IDs in a table;
/// clicks: prefix `k`, with references).
pub struct KeySpace {
    pub prefix: u8,
    pub dict: Dict,
    /// Users: the table of long IDs.
    pub long: Option<LongIds>,
    /// Clicks: IDs that are not regular are keyed by reference. Only for IDs
    /// that are never looked up by key, and whose bytes stay where they are.
    by_ref: bool,
}

impl KeySpace {
    /// Keys for IDs that are looked up (users); `long_cap`: slots for long IDs.
    pub fn new(prefix: u8, long_cap: usize) -> Self {
        KeySpace { prefix, dict: Dict::new(), long: Some(LongIds::new(long_cap)), by_ref: false }
    }

    /// Keys for IDs that are only compared, in input that stays mapped (clicks).
    pub fn by_reference(prefix: u8) -> Self {
        KeySpace { prefix, dict: Dict::new(), long: None, by_ref: true }
    }

    /// The key of a long ID's value, for a key space with a table of them.
    #[inline(always)]
    pub fn long_key(&self, hi: u64, lo: u64) -> Option<u64> {
        let slot = unsafe { self.long.as_ref().unwrap_unchecked() }.intern(hi, lo)?;
        Some(IRREGULAR | LONG | slot as u64)
    }

    #[inline]
    pub fn key(&self, b: &[u8]) -> u64 {
        if let Some(k) = raw_key(b, self.prefix) {
            return k;
        }
        if self.by_ref {
            if let Some(k) = ref_key(b.as_ptr(), b.len()) {
                return k;
            }
        } else if let Some(table) = &self.long {
            if let Some(slot) = long_value(b, self.prefix).and_then(|(hi, lo)| table.intern(hi, lo)) {
                return IRREGULAR | LONG | slot as u64;
            }
        }
        IRREGULAR | self.dict.intern(b)
    }

    /// The ID behind a key, appended to `out`.
    pub fn bytes(&self, k: u64, out: &mut Vec<u8>) {
        if k & IRREGULAR == 0 {
            out.push(self.prefix);
            out.extend_from_slice(&k.to_be_bytes()[..raw_len(k)]);
        } else if k & LONG != 0 {
            let (hi, lo) = self.long.as_ref().unwrap().get(k as u32);
            out.extend_from_slice(&long_bytes(self.prefix, hi, lo));
        } else if k & REF != 0 {
            out.extend_from_slice(unsafe { std::slice::from_raw_parts((k & REF_ADDR) as *const u8, (k >> 47) as usize & REF_MAX) });
        } else {
            out.extend_from_slice(&self.dict.get(k & !IRREGULAR));
        }
    }

    /// Byte-wise `a < b` on the IDs behind two keys.
    #[inline]
    pub fn less(&self, a: u64, b: u64) -> bool {
        if (a | b) & IRREGULAR == 0 {
            return a < b;
        }
        // (The kind bits mean something only in a key that is not regular:
        // a regular key has its ID's bytes there.)
        const KIND: u64 = IRREGULAR | LONG;
        if (a & b) & KIND == KIND {
            // Two long IDs: same prefix, and digits in the order of their values.
            let table = self.long.as_ref().unwrap();
            return table.get(a as u32) < table.get(b as u32);
        }
        let (regular, other) = if a & IRREGULAR == 0 { (a, b) } else { (b, a) };
        if regular & IRREGULAR == 0 && other & KIND == KIND {
            // A regular ID and a long one: the regular key is the ID's bytes
            // after the prefix, zero-padded to eight, and no byte of an ID is
            // zero; so it orders against the long ID's first eight digits,
            // and a regular ID equal to them is the shorter, hence smaller.
            let (hi, _) = self.long.as_ref().unwrap().get(other as u32);
            let first8 = u64::from_be_bytes(hex16(hi)[..8].try_into().unwrap());
            return if a & IRREGULAR == 0 { regular <= first8 } else { first8 < regular };
        }
        let (mut x, mut y) = (Vec::new(), Vec::new());
        self.bytes(a, &mut x);
        self.bytes(b, &mut y);
        x < y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_ids_round_trip_and_order() {
        let ks = KeySpace::new(b'u', 1 << 12);
        let a = b"u0123456789abcdef0123456789abcdef";
        let b = b"u0123456789abcdef0123456789abcdf0";
        let (ka, kb) = (ks.key(a), ks.key(b));
        assert!(ka & LONG != 0 && kb & LONG != 0 && ka != kb);
        assert_eq!(ks.key(a), ka);
        let mut out = Vec::new();
        ks.bytes(ka, &mut out);
        assert_eq!(&out, a);
        assert!(ks.less(ka, kb) && !ks.less(kb, ka) && !ks.less(ka, ka));
        // Not long: upper case, a wrong digit, another length, another prefix.
        for odd in [&b"u0123456789ABCDEF0123456789abcdef"[..], b"u0123456789abcdeg0123456789abcdef", b"u0123456789abcdef0123456789abcde", b"k0123456789abcdef0123456789abcdef"] {
            let k = ks.key(odd);
            assert!(k & IRREGULAR != 0 && k & LONG == 0);
            let mut out = Vec::new();
            ks.bytes(k, &mut out);
            assert_eq!(&out, odd);
        }
        // Against regular IDs: byte-wise.
        let short = ks.key(b"u01234567");
        assert!(ks.less(short, ka) && !ks.less(ka, short));
        let later = ks.key(b"u0123456a");
        assert!(ks.less(ka, later) && !ks.less(later, ka));
        // A value with a zero half goes to the dictionary.
        let zero = b"u00000000000000000123456789abcdef";
        assert!(ks.key(zero) & LONG == 0);
    }

    #[test]
    fn hashes_agree() {
        let id = b"e0123456789abcdef0123456789abcdef";
        let w = |k: usize| u64::from_le_bytes(id[1 + 8 * k..9 + 8 * k].try_into().unwrap());
        assert_eq!(event_hash(id), hash33(b'e', [w(0), w(1), w(2), w(3)]));
    }

    #[test]
    fn references() {
        let ks = KeySpace::by_reference(b'k');
        let text = b"k0123456789abcdef0123456789abcdef,kzz,K1";
        let (a, b, c) = (ks.key(&text[..33]), ks.key(&text[34..37]), ks.key(&text[38..]));
        assert!(a & REF != 0 && b & IRREGULAR == 0 && c & REF != 0);
        assert!(ks.less(a, b) && !ks.less(b, a) && ks.less(c, a) && ks.less(c, b) && !ks.less(a, a));
    }
}
