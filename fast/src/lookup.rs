//! The small inputs: manifest.json, products.csv, campaigns.csv.

use std::collections::HashMap;

use crate::util::{for_each_line, parse_ts, split};

/// IDs of the form `<prefix><n>` with no leading zero and `n` below this go to a
/// direct-indexed table; everything else to a hash map.
pub const DIRECT_LIMIT: usize = 1 << 22;

#[inline]
fn canonical(b: &[u8], prefix: &[u8]) -> Option<usize> {
    let p = prefix.len();
    let n = b.len();
    if n <= p || n > p + 7 || &b[..p] != prefix || b[p] == b'0' {
        return None;
    }
    let mut v = 0usize;
    for &c in &b[p..] {
        let d = c.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        v = v * 10 + d as usize;
    }
    if v < DIRECT_LIMIT {
        Some(v)
    } else {
        None
    }
}

/// Maps an ID to a dense index, with a direct table for the common shape.
pub struct IdIndex {
    prefix: &'static [u8],
    direct: Vec<u32>,
    map: HashMap<Box<[u8]>, u32>,
}

pub const ABSENT: u32 = u32::MAX;

impl IdIndex {
    fn new(prefix: &'static [u8]) -> Self {
        IdIndex { prefix, direct: Vec::new(), map: HashMap::new() }
    }

    fn insert(&mut self, id: &[u8], value: u32) {
        match canonical(id, self.prefix) {
            Some(n) => {
                if self.direct.len() <= n {
                    self.direct.resize(n + 1, ABSENT);
                }
                self.direct[n] = value;
            }
            None => {
                self.map.insert(id.into(), value);
            }
        }
    }

    #[inline]
    pub fn get(&self, id: &[u8]) -> u32 {
        match canonical(id, self.prefix) {
            Some(n) => {
                if n < self.direct.len() {
                    self.direct[n]
                } else {
                    ABSENT
                }
            }
            None => {
                if self.map.is_empty() {
                    ABSENT
                } else {
                    self.map.get(id).copied().unwrap_or(ABSENT)
                }
            }
        }
    }

    /// Direct lookup by the number after the prefix (for the SIMD fast path).
    #[inline]
    pub fn by_number(&self, n: usize) -> u32 {
        if n < self.direct.len() {
            unsafe { *self.direct.get_unchecked(n) }
        } else {
            ABSENT
        }
    }
}

pub struct Products {
    pub index: IdIndex,
    /// Category names in byte-wise order, `unknown` included: the index is the rank.
    pub categories: Vec<Box<[u8]>>,
    pub unknown: u32,
}

impl Products {
    pub fn load(data: &[u8]) -> Products {
        // One pass: number the distinct categories in order of appearance (there
        // are few, and consecutive products often share one), remember each
        // product's number; then rank the names and renumber.
        let mut names: Vec<&[u8]> = vec![b"unknown"];
        let mut seen: HashMap<&[u8], u32> = HashMap::new();
        seen.insert(b"unknown", 0);
        let mut rows: Vec<(&[u8], u32)> = Vec::with_capacity(data.len() / 12);
        let (mut last_name, mut last_id): (&[u8], u32) = (b"unknown", 0);
        for_each_line(data, 0, data.len(), |line| {
            if let Some([id, cat]) = split::<2>(line) {
                if cat != last_name {
                    last_name = cat;
                    last_id = *seen.entry(cat).or_insert_with(|| {
                        names.push(cat);
                        names.len() as u32 - 1
                    });
                }
                rows.push((id, last_id));
            }
        });
        let mut order: Vec<u32> = (0..names.len() as u32).collect();
        order.sort_unstable_by_key(|&i| names[i as usize]);
        let mut rank = vec![0u32; names.len()];
        for (r, &i) in order.iter().enumerate() {
            rank[i as usize] = r as u32;
        }
        let mut index = IdIndex::new(b"p");
        for &(id, cat) in &rows {
            index.insert(id, rank[cat as usize]);
        }
        Products {
            index,
            unknown: rank[0],
            categories: order.iter().map(|&i| names[i as usize].into()).collect(),
        }
    }

    #[inline]
    pub fn category(&self, product: &[u8]) -> u32 {
        let c = self.index.get(product);
        if c == ABSENT {
            self.unknown
        } else {
            c
        }
    }
}

pub struct Campaigns {
    pub index: IdIndex,
    /// Campaign IDs and channels by index; the last entry is `organic`.
    pub ids: Vec<Box<[u8]>>,
    pub channels: Vec<Box<[u8]>>,
    pub organic: u32,
}

impl Campaigns {
    pub fn load(data: &[u8]) -> Campaigns {
        let mut index = IdIndex::new(b"c_");
        let mut ids: Vec<Box<[u8]>> = Vec::new();
        let mut channels: Vec<Box<[u8]>> = Vec::new();
        let mut seen: HashMap<Box<[u8]>, u32> = HashMap::new();
        for_each_line(data, 0, data.len(), |line| {
            if let Some([id, channel]) = split::<2>(line) {
                if !seen.contains_key(id) {
                    let i = ids.len() as u32;
                    seen.insert(id.into(), i);
                    index.insert(id, i);
                    ids.push(id.into());
                    channels.push(channel.into());
                }
            }
        });
        let organic = ids.len() as u32;
        ids.push((&b"organic"[..]).into());
        channels.push((&b"organic"[..]).into());
        Campaigns { index, ids, channels, organic }
    }
}

/// `(window_start, window_end)` in epoch milliseconds.
pub fn manifest(text: &[u8]) -> Option<(i64, i64)> {
    fn value(text: &[u8], key: &[u8]) -> Option<i64> {
        let at = text.windows(key.len()).position(|w| w == key)? + key.len();
        let rest = &text[at..];
        let colon = rest.iter().position(|&c| c == b':')?;
        let rest = &rest[colon + 1..];
        let open = rest.iter().position(|&c| c == b'"')?;
        let rest = &rest[open + 1..];
        let close = rest.iter().position(|&c| c == b'"')?;
        parse_ts(&rest[..close])
    }
    Some((value(text, b"\"window_start\"")?, value(text, b"\"window_end\"")?))
}
