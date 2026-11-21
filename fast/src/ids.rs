//! Exact integer keys for IDs.
//!
//! A character of `[0-9a-z]` has a 6-bit code: bits 0-4 and bit 6 of its ASCII
//! byte (`pext(byte, 0x5F)`), so digits are 16-25 and letters 33-58. The code is
//! never 0, is injective, and keeps byte order.
//!
//! An ID of 1 to 9 such characters ("regular") packs into 54 bits, 6 per
//! character, left-aligned, so the integer order of two keys is the byte-wise
//! order of the IDs. Every other ID ("irregular": upper case, spaces, longer,
//! ...) is interned in a dictionary and keyed by its index with the top bit set.
//! The mapping is injective either way, so equal keys mean equal IDs.

use std::collections::HashMap;
use std::sync::Mutex;

pub const IRREGULAR: u64 = 1 << 63;
/// Event keys are 49 bits: 48 for `e` + up to 8 regular characters (6 bits
/// each, first character lowest), or this flag plus a dictionary index.
pub const EVENT_IRREGULAR: u64 = 1 << 48;
pub const EVENT_KEY_BITS: u32 = 49;

const fn build_codes() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut c = 0usize;
    while c < 256 {
        let b = c as u8;
        if (b >= b'0' && b <= b'9') || (b >= b'a' && b <= b'z') {
            t[c] = (b & 0x1F) | ((b & 0x40) >> 1);
        }
        c += 1;
    }
    t
}
pub static CODE: [u8; 256] = build_codes();

#[inline(always)]
fn decode(code: u64) -> u8 {
    ((code & 0x1F) | ((code & 0x20) << 1) | 0x20) as u8
}

/// The 54-bit order-preserving key of a regular ID, or `None`.
#[inline]
pub fn key9(b: &[u8]) -> Option<u64> {
    let n = b.len();
    if n == 0 || n > 9 {
        return None;
    }
    let mut k = 0u64;
    let mut i = 0;
    while i < n {
        let v = CODE[b[i] as usize] as u64;
        if v == 0 {
            return None;
        }
        k |= v << (6 * (8 - i));
        i += 1;
    }
    Some(k)
}

pub fn key9_decode(k: u64, out: &mut Vec<u8>) {
    for i in 0..9 {
        let v = (k >> (6 * (8 - i))) & 63;
        if v == 0 {
            break;
        }
        out.push(decode(v));
    }
}

/// The 48-bit key of a regular event ID (`e` then up to 8 of `[0-9a-z]`), or `None`.
#[inline]
pub fn event_key(b: &[u8]) -> Option<u64> {
    let n = b.len();
    if n == 0 || n > 9 || b[0] != b'e' {
        return None;
    }
    let mut k = 0u64;
    let mut i = 1;
    while i < n {
        let v = CODE[b[i] as usize] as u64;
        if v == 0 {
            return None;
        }
        k |= v << (6 * (i - 1));
        i += 1;
    }
    Some(k)
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

/// Keys for user IDs and click IDs: regular 54-bit keys or interned.
pub struct KeySpace {
    pub dict: Dict,
}

impl KeySpace {
    pub fn new() -> Self {
        KeySpace { dict: Dict::new() }
    }

    #[inline]
    pub fn key(&self, b: &[u8]) -> u64 {
        match key9(b) {
            Some(k) => k,
            None => IRREGULAR | self.dict.intern(b),
        }
    }

    pub fn bytes(&self, k: u64, out: &mut Vec<u8>) {
        if k & IRREGULAR == 0 {
            key9_decode(k, out);
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
