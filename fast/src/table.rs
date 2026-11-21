//! The global user table: user key -> slot, and a lock-free union-find over
//! slots that joins linked IDs into people.
//!
//! A person is the slot of the root of their set. Roots are always the
//! byte-wise smallest ID of the set (unions point the larger root at the
//! smaller), so the root's key is the person's name.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use crate::sys::map_anon;

pub struct Table {
    keys: *mut AtomicU64,
    /// 0 = root, else parent slot + 1.
    parent: *mut AtomicU32,
    pub cap: usize,
    pub overflow: AtomicBool,
}
unsafe impl Send for Table {}
unsafe impl Sync for Table {}

impl Table {
    pub fn new(cap: usize) -> Table {
        Table {
            keys: map_anon(cap * 8, true) as *mut AtomicU64,
            parent: map_anon(cap * 4, true) as *mut AtomicU32,
            cap,
            overflow: AtomicBool::new(false),
        }
    }

    #[inline(always)]
    fn home(&self, key: u64) -> usize {
        ((key.wrapping_mul(0x9E37_79B9_7F4A_7C15) as u128 * self.cap as u128) >> 64) as usize
    }

    #[inline(always)]
    pub fn prefetch(&self, key: u64) {
        unsafe {
            core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(
                self.keys.add(self.home(key)) as *const i8,
            )
        };
    }

    /// The slot of `key`, inserting it if new. Safe to call from many threads.
    #[inline]
    pub fn insert(&self, key: u64) -> u32 {
        let mut i = self.home(key);
        let mut probes = 0usize;
        loop {
            let slot = unsafe { &*self.keys.add(i) };
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
        unsafe { (*self.keys.add(slot as usize)).load(Relaxed) }
    }

    #[inline(always)]
    fn par(&self, slot: u32) -> &AtomicU32 {
        unsafe { &*self.parent.add(slot as usize) }
    }

    /// The person a slot belongs to. Valid once `compress` has run.
    #[inline(always)]
    pub fn person(&self, slot: u32) -> u32 {
        let p = self.par(slot).load(Relaxed);
        if p == 0 {
            slot
        } else {
            p - 1
        }
    }

    #[inline(always)]
    pub fn prefetch_person(&self, slot: u32) {
        unsafe {
            core::arch::x86_64::_mm_prefetch::<{ core::arch::x86_64::_MM_HINT_T0 }>(
                self.parent.add(slot as usize) as *const i8,
            )
        };
    }

    pub fn find(&self, mut x: u32) -> u32 {
        loop {
            let p = self.par(x).load(Relaxed);
            if p == 0 {
                return x;
            }
            let gp = self.par(p - 1).load(Relaxed);
            if gp == 0 {
                return p - 1;
            }
            // Path halving; losing the race just leaves a longer path.
            let _ = self.par(x).compare_exchange(p, gp, Relaxed, Relaxed);
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
            if self.par(child).compare_exchange(0, root + 1, Relaxed, Relaxed).is_ok() {
                return;
            }
        }
    }

    /// Point every slot in `[from, to)` straight at its root. Run after all
    /// unions; ranges may be compressed by different threads at once.
    pub fn compress(&self, from: usize, to: usize) {
        for i in from..to {
            let p = self.par(i as u32).load(Relaxed);
            if p != 0 {
                let r = self.find(p - 1);
                if r + 1 != p {
                    self.par(i as u32).store(r + 1, Relaxed);
                }
            }
        }
    }
}
