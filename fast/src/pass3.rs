//! Pass 3: one person partition -> sessions, attribution, and output rows.

use crate::parse::{CAMPAIGN_BITS, LOCAL_BITS, TS_MASK};
use crate::util::{DAY_MS, GAP_MS, REACH_MS};
use crate::Ctx;

/// Per (campaign, day) counters.
#[derive(Clone, Copy, Default)]
pub struct CampCell {
    pub sessions: u32,
    pub purchasing: u32,
    pub assisted: u32,
    pub revenue: u64,
}

/// user_daily rows produced by one thread, column by column.
#[derive(Default)]
pub struct Rows {
    pub key: Vec<u64>,
    pub day: Vec<i32>,
    pub sessions: Vec<i32>,
    pub events: Vec<i32>,
    pub revenue: Vec<i64>,
    /// Category index, `u32::MAX` for null.
    pub cat: Vec<u32>,
}

pub struct Scratch {
    ev: Vec<u64>,
    pu: Vec<u64>,
    cl: Vec<u64>,
    tmp: Vec<u64>,
    starts: Vec<u32>,
    sdays: Vec<u32>,
    dfirst: Vec<u32>,
    stamp: Vec<u32>,
    tag: u32,
    cat_sum: Vec<u64>,
    touched: Vec<u32>,
    pub rows: Rows,
    pub acc: Vec<CampCell>,
}

impl Scratch {
    pub fn new(ctx: &Ctx) -> Scratch {
        Scratch {
            ev: Vec::new(),
            pu: Vec::new(),
            cl: Vec::new(),
            tmp: Vec::new(),
            starts: Vec::new(),
            sdays: Vec::new(),
            dfirst: Vec::new(),
            stamp: vec![0; ctx.campaigns.ids.len()],
            tag: 0,
            cat_sum: vec![0; ctx.products.categories.len()],
            touched: Vec::new(),
            rows: Rows::default(),
            acc: vec![CampCell::default(); ctx.campaigns.ids.len() * ctx.ndays],
        }
    }
}

#[inline(always)]
fn click_ts(w0: u64) -> u32 {
    ((w0 >> CAMPAIGN_BITS) & TS_MASK) as u32
}
#[inline(always)]
fn click_campaign(w0: u64) -> usize {
    (w0 & ((1 << CAMPAIGN_BITS) - 1)) as usize
}

fn pairs(v: &mut [u64]) -> &mut [[u64; 2]] {
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut [u64; 2], v.len() / 2) }
}

pub fn run(ctx: &Ctx, sc: &mut Scratch, p: usize) {
    sc.ev.clear();
    sc.pu.clear();
    sc.cl.clear();
    for t in 0..ctx.nthreads {
        ctx.s(t).copy_part(&ctx.pool, p, &mut sc.ev);
    }
    if sc.ev.is_empty() {
        return;
    }
    for t in 0..ctx.nthreads {
        ctx.ps(t).copy_part(&ctx.pool, p, &mut sc.pu);
        ctx.c(t).copy_part(&ctx.pool, p, &mut sc.cl);
    }
    let mut ev = std::mem::take(&mut sc.ev);
    let mut pu = std::mem::take(&mut sc.pu);
    let mut cl = std::mem::take(&mut sc.cl);
    ev.sort_unstable();
    {
        let pu = pairs(&mut pu);
        pu.sort_unstable_by_key(|r| r[0]);
        let cl = pairs(&mut cl);
        cl.sort_unstable_by_key(|r| r[0]);
        let n = ev.len();
        let (mut i, mut pi, mut ci) = (0, 0, 0);
        while i < n {
            let local = ev[i] >> 31;
            let mut j = i + 1;
            while j < n && ev[j] >> 31 == local {
                j += 1;
            }
            let mut pj = pi;
            while pj < pu.len() && pu[pj][0] >> 31 == local {
                pj += 1;
            }
            while ci < cl.len() && cl[ci][0] >> 52 < local {
                ci += 1;
            }
            let mut cj = ci;
            while cj < cl.len() && cl[cj][0] >> 52 == local {
                cj += 1;
            }
            let slot = (p << LOCAL_BITS) as u32 | local as u32;
            person(ctx, sc, slot, &ev[i..j], &pu[pi..pj], &cl[ci..cj]);
            i = j;
            pi = pj;
            ci = cj;
        }
    }
    sc.ev = ev;
    sc.pu = pu;
    sc.cl = cl;
}

fn person(ctx: &Ctx, sc: &mut Scratch, slot: u32, ev: &[u64], pu: &[[u64; 2]], cl: &[[u64; 2]]) {
    let key = ctx.table.key(slot);
    let ndays = ctx.ndays;
    let organic = ctx.campaigns.organic as usize;
    let n = ev.len();
    let nc = cl.len();
    // With many sessions and many clicks, per-session scans of the click window
    // are quadratic; assists are then counted per campaign instead (below).
    let big = (n as u64) * (nc as u64) > 20_000;
    sc.starts.clear();
    sc.sdays.clear();

    let (mut lo, mut hi, mut pk) = (0usize, 0usize, 0usize);
    let mut cur_day = u32::MAX;
    let (mut d_sessions, mut d_events, mut d_revenue) = (0i32, 0i32, 0i64);
    let mut k = 0;
    while k < n {
        let start = (ev[k] & TS_MASK) as u32;
        let mut last = start;
        let mut m = k + 1;
        while m < n {
            let t = (ev[m] & TS_MASK) as u32;
            if t - last > GAP_MS {
                break;
            }
            last = t;
            m += 1;
        }
        let day = ((start as u64 + ctx.day_off) / DAY_MS as u64) as u32;
        if day != cur_day {
            if cur_day != u32::MAX {
                flush_day(ctx, sc, key, cur_day, d_sessions, d_events, d_revenue);
            }
            cur_day = day;
            d_sessions = 0;
            d_events = 0;
            d_revenue = 0;
        }
        // Purchases of this session: every purchase with ts in [start, last].
        let mut revenue = 0u64;
        let mut purchasing = false;
        while pk < pu.len() && (pu[pk][0] & TS_MASK) as u32 <= last {
            let w = pu[pk][1];
            let price = w >> 32;
            let cat = w as u32;
            revenue += price;
            purchasing = true;
            let cell = &mut sc.cat_sum[cat as usize];
            if *cell == 0 {
                sc.touched.push(cat);
            }
            *cell += price;
            pk += 1;
        }
        d_sessions += 1;
        d_events += (m - k) as i32;
        d_revenue += revenue as i64;

        // Credit: the latest click in [start - 7 days, start]; ties to the smallest click_id.
        while hi < nc && click_ts(cl[hi][0]) <= start {
            hi += 1;
        }
        while lo < hi && click_ts(cl[lo][0]) as i64 + REACH_MS < start as i64 {
            lo += 1;
        }
        let campaign = if hi > lo {
            let tmax = click_ts(cl[hi - 1][0]);
            let mut best = hi - 1;
            let mut j = hi - 1;
            while j > lo && click_ts(cl[j - 1][0]) == tmax {
                j -= 1;
                if ctx.clicks.less(cl[j][1], cl[best][1]) {
                    best = j;
                }
            }
            click_campaign(cl[best][0])
        } else {
            organic
        };
        let cell = &mut sc.acc[campaign * ndays + day as usize];
        cell.sessions += 1;
        cell.purchasing += purchasing as u32;
        cell.revenue += revenue;

        if big {
            sc.starts.push(start);
            sc.sdays.push(day);
        } else if hi > lo {
            // Assists: each distinct campaign in the window, once.
            sc.tag = sc.tag.wrapping_add(1);
            if sc.tag == 0 {
                sc.stamp.iter_mut().for_each(|s| *s = 0);
                sc.tag = 1;
            }
            for c in &cl[lo..hi] {
                let camp = click_campaign(c[0]);
                if sc.stamp[camp] != sc.tag {
                    sc.stamp[camp] = sc.tag;
                    sc.acc[camp * ndays + day as usize].assisted += 1;
                }
            }
        }
        k = m;
    }
    flush_day(ctx, sc, key, cur_day, d_sessions, d_events, d_revenue);
    if big {
        assists_by_campaign(sc, cl, ndays);
    }
}

#[inline]
fn flush_day(ctx: &Ctx, sc: &mut Scratch, key: u64, day: u32, sessions: i32, events: i32, revenue: i64) {
    // Top category: highest summed price, ties to the smallest name (= smallest index).
    let mut best = u32::MAX;
    let mut best_sum = 0u64;
    for &cat in &sc.touched {
        let sum = sc.cat_sum[cat as usize];
        if sum > best_sum || (sum == best_sum && cat < best) {
            best = cat;
            best_sum = sum;
        }
        sc.cat_sum[cat as usize] = 0;
    }
    sc.touched.clear();
    let r = &mut sc.rows;
    r.key.push(key);
    r.day.push((ctx.day0 + day as i64) as i32);
    r.sessions.push(sessions);
    r.events.push(events);
    r.revenue.push(revenue);
    r.cat.push(best);
}

/// Assists for a person with many sessions and clicks. A campaign assists a
/// session iff the session start lies in `[t, t + 7 days]` for one of the
/// campaign's clicks at `t`; merge those intervals per campaign and count the
/// session starts they cover, day by day.
fn assists_by_campaign(sc: &mut Scratch, cl: &[[u64; 2]], ndays: usize) {
    let starts = &sc.starts[..];
    let sdays = &sc.sdays[..];
    // dfirst[d] = first session index whose day is >= d.
    sc.dfirst.clear();
    sc.dfirst.resize(ndays + 1, starts.len() as u32);
    for (i, &d) in sdays.iter().enumerate().rev() {
        sc.dfirst[d as usize] = i as u32;
    }
    for d in (0..ndays).rev() {
        if sc.dfirst[d] > sc.dfirst[d + 1] {
            sc.dfirst[d] = sc.dfirst[d + 1];
        }
    }
    sc.tmp.clear();
    sc.tmp.extend(cl.iter().map(|c| (click_campaign(c[0]) as u64) << 32 | click_ts(c[0]) as u64));
    sc.tmp.sort_unstable();
    let tmp = &sc.tmp[..];
    let reach = REACH_MS as u64;
    let mut i = 0;
    while i < tmp.len() {
        let camp = (tmp[i] >> 32) as usize;
        let a = tmp[i] & 0xFFFF_FFFF;
        let mut b = a + reach;
        i += 1;
        while i < tmp.len() && (tmp[i] >> 32) as usize == camp && (tmp[i] & 0xFFFF_FFFF) <= b {
            b = (tmp[i] & 0xFFFF_FFFF) + reach;
            i += 1;
        }
        let x = starts.partition_point(|&s| (s as u64) < a);
        let y = starts.partition_point(|&s| (s as u64) <= b);
        if y > x {
            for d in sdays[x] as usize..=sdays[y - 1] as usize {
                let from = (sc.dfirst[d] as usize).max(x);
                let to = (sc.dfirst[d + 1] as usize).min(y);
                if to > from {
                    sc.acc[camp * ndays + d].assisted += (to - from) as u32;
                }
            }
        }
    }
}
