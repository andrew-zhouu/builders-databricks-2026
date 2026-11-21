//! The global user table: user key -> slot, and a lock-free union-find over
//! slots that joins linked IDs into people.
//!
//! A slot is 16 bytes: the key and the slot's parent side by side, so finding a
//! user and the person they belong to is a single cache miss.
//!
//! A person is the slot of the root of their set. Roots are always the
//! byte-wise smallest ID of the set (unions point the larger root at the
//! smaller), so the root's key is the person's name.

use core::arch::x86_64::{_mm_prefetch, _MM_HINT_T0};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use crate::sys::map_anon;

#[repr(C)]
struct Slot {
    /// 0 = empty.
    key: AtomicU64,
    /// 0 = root, else parent slot + 1.
    parent: AtomicU32,
    _pad: u32,
}

pub struct Table {
    slots: *mut Slot,
    pub cap: usize,
    pub overflow: AtomicBool,
    /// Linked IDs in the table once `compress` has run (diagnostics).
    pub used: std::sync::atomic::AtomicUsize,
}
unsafe impl Send for Table {}
unsafe impl Sync for Table {}

impl Table {
    pub fn new(cap: usize) -> Table {
        Table {
            slots: map_anon(cap * 16, true) as *mut Slot,
            cap,
            overflow: AtomicBool::new(false),
            used: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn slot(&self, i: u32) -> &Slot {
        unsafe { &*self.slots.add(i as usize) }
    }

    #[inline(always)]
    fn home(&self, key: u64) -> usize {
        ((key.wrapping_mul(0x9E37_79B9_7F4A_7C15) as u128 * self.cap as u128) >> 64) as usize
    }

    /// Bring the home slot of `key` into cache.
    #[inline(always)]
    pub fn prefetch(&self, key: u64) {
        unsafe { _mm_prefetch::<_MM_HINT_T0>(self.slots.add(self.home(key)) as *const i8) };
    }

    #[inline(always)]
    pub fn prefetch_slot(&self, slot: u32) {
        unsafe { _mm_prefetch::<_MM_HINT_T0>(self.slots.add(slot as usize) as *const i8) };
    }

    /// The slot of `key`, inserting it if new. Safe to call from many threads.
    #[inline(always)]
    pub fn insert(&self, key: u64) -> u32 {
        let mut i = self.home(key);
        let mut probes = 0usize;
        loop {
            let slot = unsafe { &(*self.slots.add(i)).key };
            let cur = slot.load(Relaxed);
            if cur == key {
                return i as u32;
            }
            if cur == 0 {
                match slot.compare_exchange(0, key, Relaxed, Relaxed) {
                    Ok(_) => return i as u32,
                    Err(actual) => {
                        if actual == key {
                            return i as u32;
                        }
                    }
                }
            }
            i += 1;
            if i == self.cap {
                i = 0;
            }
            probes += 1;
            if probes > self.cap {
                self.overflow.store(true, Relaxed);
                return 0;
            }
        }
    }

    #[inline(always)]
    pub fn key(&self, slot: u32) -> u64 {
        self.slot(slot).key.load(Relaxed)
    }

    /// The person a slot belongs to. Valid once `compress` has run.
    #[inline(always)]
    pub fn person(&self, slot: u32) -> u32 {
        let p = self.slot(slot).parent.load(Relaxed);
        if p == 0 {
            slot
        } else {
            p - 1
        }
    }

    pub fn find(&self, mut x: u32) -> u32 {
        loop {
            let p = self.slot(x).parent.load(Relaxed);
            if p == 0 {
                return x;
            }
            let gp = self.slot(p - 1).parent.load(Relaxed);
            if gp == 0 {
                return p - 1;
            }
            // Path halving; losing the race just leaves a longer path.
            let _ = self.slot(x).parent.compare_exchange(p, gp, Relaxed, Relaxed);
            x = gp - 1;
        }
    }

    /// Join the sets of two slots. `less(ka, kb)` is the byte-wise ID order.
    pub fn union(&self, a: u32, b: u32, less: impl Fn(u64, u64) -> bool) {
        let (mut a, mut b) = (a, b);
        loop {
            a = self.find(a);
            b = self.find(b);
            if a == b {
                return;
            }
            let (root, child) = if less(self.key(a), self.key(b)) { (a, b) } else { (b, a) };
            if self.slot(child).parent.compare_exchange(0, root + 1, Relaxed, Relaxed).is_ok() {
                return;
            }
        }
    }

    /// Give the pages of slots `[from, to)` back to the kernel (teardown only).
    pub fn zap(&self, from: usize, to: usize) {
        crate::sys::zap(unsafe { self.slots.add(from) } as *mut u8, (to - from) * 16);
    }

    /// Point every slot in `[from, to)` straight at its root. Run after all
    /// unions; ranges may be compressed by different threads at once. Parents
    /// are prefetched a few slots ahead of the walk.
    pub fn compress(&self, from: usize, to: usize) {
        const AHEAD: usize = 16;
        let mut used = 0usize;
        for i in from..to {
            used += (self.slot(i as u32).key.load(Relaxed) != 0) as usize;
            if i + AHEAD < to {
                let p = self.slot((i + AHEAD) as u32).parent.load(Relaxed);
                if p != 0 {
                    self.prefetch_slot(p - 1);
                }
            }
            let p = self.slot(i as u32).parent.load(Relaxed);
            if p != 0 {
                let r = self.find(p - 1);
                if r + 1 != p {
                    self.slot(i as u32).parent.store(r + 1, Relaxed);
                }
            }
        }
        self.used.fetch_add(used, Relaxed);
    }
}
