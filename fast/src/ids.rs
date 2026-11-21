//! Exact integer keys for user and click IDs, and fingerprints for event IDs.
//!
//! **User and click IDs.** An ID is *regular* when it is its usual prefix
//! letter (`u` or `k`) followed by 1 to 8 bytes that are all ASCII and not NUL.
//! Its key is those bytes as a big-endian integer, zero-padded on the right:
//! injective (a shorter ID never equals a longer one because no byte is zero),
//! never 0, top bit clear, and ordered exactly like the IDs byte-wise. Every
//! other ID (other first letter, longer, non-ASCII, ...) is interned in a
//! dictionary and keyed by its index with the top bit set. Equal keys mean
//! equal IDs either way.
//!
//! **Event IDs** are only ever compared for equality, and candidates for
//! equality are confirmed by reading the IDs again from the input, so an event
//! ID needs no exact key: `event_hash` is an ordinary 64-bit hash of its bytes.

use std::collections::HashMap;
use std::sync::Mutex;

pub const IRREGULAR: u64 = 1 << 63;
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
    use core::arch::x86_64::_bzhi_u64;
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

/// A 64-bit hash of an event ID. Not exact: equal hashes are confirmed by
/// comparing the IDs themselves.
#[inline]
pub fn event_hash(b: &[u8]) -> u64 {
    let n = b.len();
    if n <= 9 {
        // The usual shape, a letter and up to 8 more bytes: the bytes after the
        // first are the hash input (IDs that differ only in their first byte
        // share a hash, which is allowed).
        let mut w = [0u8; 8];
        if n > 1 {
            w[..n - 1].copy_from_slice(&b[1..]);
        }
        return mix9(u64::from_le_bytes(w));
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

/// The hash of an ID of at most 9 bytes: `x` holds its bytes after the first,
/// little-endian and zero-padded.
#[inline(always)]
pub fn mix9(x: u64) -> u64 {
    let a = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let b = (a ^ (a >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    b ^ (b >> 32)
}

/// The hash of an ID of `n <= 16` bytes whose bytes, zero-padded, are `lo` and `hi`.
#[inline(always)]
pub fn mix16(lo: u64, hi: u64, n: usize) -> u64 {
    finish((lo ^ hi.rotate_left(29) ^ (n as u64) << 56).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ hi)
}

#[inline(always)]
fn finish(mut h: u64) -> u64 {
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 32)
}

/// Interns irregular IDs. Rare by construction, so one lock is fine.
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

/// Keys for one kind of ID (users: prefix `u`; clicks: prefix `k`).
pub struct KeySpace {
    pub prefix: u8,
    pub dict: Dict,
}

impl KeySpace {
    pub fn new(prefix: u8) -> Self {
        KeySpace { prefix, dict: Dict::new() }
    }

    #[inline]
    pub fn key(&self, b: &[u8]) -> u64 {
        match raw_key(b, self.prefix) {
            Some(k) => k,
            None => IRREGULAR | self.dict.intern(b),
        }
    }

    pub fn bytes(&self, k: u64, out: &mut Vec<u8>) {
        if k & IRREGULAR == 0 {
            out.push(self.prefix);
            out.extend_from_slice(&k.to_be_bytes()[..raw_len(k)]);
        } else {
            out.extend_from_slice(&self.dict.get(k & !IRREGULAR));
        }
    }

    /// Byte-wise `a < b` on the IDs behind two keys.
    #[inline]
    pub fn less(&self, a: u64, b: u64) -> bool {
        if (a | b) & IRREGULAR == 0 {
            a < b
        } else {
            let (mut x, mut y) = (Vec::new(), Vec::new());
            self.bytes(a, &mut x);
            self.bytes(b, &mut y);
            x < y
        }
    }
}
