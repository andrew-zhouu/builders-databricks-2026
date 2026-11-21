//! The plain implementation of rules.md: single-threaded, hash maps of byte
//! strings, no packed layouts and so no limits on IDs, window length or counts.
//!
//! It is the fallback whenever the input does not fit the fast path, and the
//! oracle the fast path is diffed against at sizes the Python reference cannot
//! reach (`--slow`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::parquet::{self, Column, Kind};
use crate::util::{for_each_line, parse_price, parse_ts, split, DAY_MS, REACH_MS};

struct Event {
    ingest: i64,
    valid: bool,
    user: u32,
    ts: i64,
    purchase: bool,
    price: u32,
    category: u32,
}

#[derive(Default)]
struct Users {
    index: HashMap<Box<[u8]>, u32>,
    names: Vec<Box<[u8]>>,
    parent: Vec<u32>,
}

impl Users {
    fn id(&mut self, name: &[u8]) -> u32 {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.names.len() as u32;
        self.names.push(name.into());
        self.parent.push(i);
        self.index.insert(name.into(), i);
        i
    }

    fn find(&mut self, mut x: u32) -> u32 {
        let mut root = x;
        while self.parent[root as usize] != root {
            root = self.parent[root as usize];
        }
        while self.parent[x as usize] != root {
            let next = self.parent[x as usize];
            self.parent[x as usize] = root;
            x = next;
        }
        root
    }

    /// The root of a set is always its byte-wise smallest ID: the person's name.
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            if self.names[ra as usize] < self.names[rb as usize] {
                self.parent[rb as usize] = ra;
            } else {
                self.parent[ra as usize] = rb;
            }
        }
    }
}

fn csv_files(dir: &Path) -> Vec<std::path::PathBuf> {
    crate::list_csv(dir)
}

pub fn run(data_dir: &Path, out_dir: &Path) {
    let (ws, we) = crate::lookup::manifest(&std::fs::read(data_dir.join("manifest.json")).expect("manifest.json"))
        .expect("manifest window");

    // products -> category index (categories sorted byte-wise, `unknown` included)
    let products_raw = std::fs::read(data_dir.join("products.csv")).unwrap_or_default();
    let mut product_cat: HashMap<&[u8], &[u8]> = HashMap::new();
    for_each_line(&products_raw, 0, products_raw.len(), |line| {
        if let Some([id, cat]) = split::<2>(line) {
            product_cat.insert(id, cat);
        }
    });
    let mut categories: Vec<&[u8]> = product_cat.values().copied().collect();
    categories.push(b"unknown");
    categories.sort_unstable();
    categories.dedup();
    let cat_index: HashMap<&[u8], u32> = categories.iter().enumerate().map(|(i, c)| (*c, i as u32)).collect();
    let unknown = cat_index[&b"unknown"[..]];

    let campaigns_raw = std::fs::read(data_dir.join("campaigns.csv")).unwrap_or_default();
    let mut campaign_index: HashMap<&[u8], u32> = HashMap::new();
    let mut campaign_ids: Vec<&[u8]> = Vec::new();
    let mut campaign_channels: Vec<&[u8]> = Vec::new();
    for_each_line(&campaigns_raw, 0, campaigns_raw.len(), |line| {
        if let Some([id, channel]) = split::<2>(line) {
            if !campaign_index.contains_key(id) {
                campaign_index.insert(id, campaign_ids.len() as u32);
                campaign_ids.push(id);
                campaign_channels.push(channel);
            }
        }
    });
    let organic = campaign_ids.len() as u32;
    campaign_ids.push(b"organic");
    campaign_channels.push(b"organic");

    // ---- linked IDs
    let mut users = Users::default();
    for path in csv_files(&data_dir.join("links")) {
        let data = std::fs::read(&path).expect("read links");
        for_each_line(&data, 0, data.len(), |line| {
            if let Some([a, b]) = split::<2>(line) {
                if !a.is_empty() && !b.is_empty() {
                    let (ia, ib) = (users.id(a), users.id(b));
                    users.union(ia, ib);
                }
            }
        });
    }

    // ---- events: dedup on event_id (smallest ingest_ts), validity of the kept copy
    let mut events: HashMap<Box<[u8]>, Event> = HashMap::new();
    for path in csv_files(&data_dir.join("events")) {
        let data = std::fs::read(&path).expect("read events");
        for_each_line(&data, 0, data.len(), |line| {
            let Some(f) = split::<7>(line) else { return };
            if f[0].is_empty() {
                return;
            }
            let Some(ingest) = parse_ts(f[3]) else { return };
            if let Some(prev) = events.get(f[0]) {
                if prev.ingest <= ingest {
                    return;
                }
            }
            let mut valid = !f[1].is_empty() && !f[5].is_empty();
            let mut ts = 0;
            match parse_ts(f[2]) {
                Some(t) if t >= ws && t < we => ts = t,
                _ => valid = false,
            }
            let purchase = match f[4] {
                b"view" | b"cart" => false,
                b"purchase" => true,
                _ => {
                    valid = false;
                    false
                }
            };
            let mut price = 0;
            if purchase {
                match parse_price(f[6]) {
                    Some(p) => price = p,
                    None => valid = false,
                }
            }
            if valid && ingest - ts > DAY_MS {
                valid = false;
            }
            let (user, category) = if valid {
                (users.id(f[1]), product_cat.get(f[5]).map_or(unknown, |c| cat_index[c]))
            } else {
                (0, 0)
            };
            events.insert(f[0].into(), Event { ingest, valid, user, ts, purchase, price, category });
        });
    }
    // (ts, purchase, price, category) per person
    let mut per_person: HashMap<u32, Vec<(i64, bool, u32, u32)>> = HashMap::new();
    for e in events.values() {
        if e.valid {
            let person = users.find(e.user);
            per_person.entry(person).or_default().push((e.ts, e.purchase, e.price, e.category));
        }
    }
    drop(events);

    // ---- clicks: dedup on click_id, validate; (ts, click_id, campaign) per person
    let mut seen: HashSet<Box<[u8]>> = HashSet::new();
    let mut person_clicks: HashMap<u32, Vec<(i64, Box<[u8]>, u32)>> = HashMap::new();
    for path in csv_files(&data_dir.join("clicks")) {
        let data = std::fs::read(&path).expect("read clicks");
        for_each_line(&data, 0, data.len(), |line| {
            let Some(f) = split::<4>(line) else { return };
            if f[0].is_empty() || !seen.insert(f[0].into()) {
                return;
            }
            if f[1].is_empty() {
                return;
            }
            let ts = match parse_ts(f[3]) {
                Some(t) if t >= ws - REACH_MS && t < we => t,
                _ => return,
            };
            let Some(&campaign) = campaign_index.get(f[2]) else { return };
            let user = users.id(f[1]);
            let person = users.find(user);
            person_clicks.entry(person).or_default().push((ts, f[0].into(), campaign));
        });
    }
    drop(seen);

    // ---- sessions, attribution, aggregation
    // (person, day) -> (sessions, events, revenue, category sums)
    let mut user_daily: HashMap<(u32, i64), (i64, i64, i64, HashMap<u32, i64>)> = HashMap::new();
    // (campaign, day) -> (sessions, purchasing, revenue, assisted)
    let mut campaign_daily: HashMap<(u32, i64), (i64, i64, i64, i64)> = HashMap::new();
    let no_clicks: Vec<(i64, Box<[u8]>, u32)> = Vec::new();
    for (&person, list) in per_person.iter_mut() {
        list.sort_unstable_by_key(|e| e.0);
        let clicks = match person_clicks.get_mut(&person) {
            Some(c) => {
                c.sort_unstable_by(|a, b| a.0.cmp(&b.0));
                &*c
            }
            None => &no_clicks,
        };
        let mut i = 0;
        while i < list.len() {
            let start = list[i].0;
            let mut j = i + 1;
            while j < list.len() && list[j].0 - list[j - 1].0 <= 1_800_000 {
                j += 1;
            }
            let session = &list[i..j];
            let day = start.div_euclid(DAY_MS);
            let revenue: i64 = session.iter().filter(|e| e.1).map(|e| e.2 as i64).sum();
            let purchasing = session.iter().any(|e| e.1);
            let row = user_daily.entry((person, day)).or_default();
            row.0 += 1;
            row.1 += session.len() as i64;
            row.2 += revenue;
            for e in session.iter().filter(|e| e.1) {
                *row.3.entry(e.3).or_default() += e.2 as i64;
            }
            // clicks with start - 7 days <= ts <= start
            let from = clicks.partition_point(|c| c.0 < start - REACH_MS);
            let to = clicks.partition_point(|c| c.0 <= start);
            let window = &clicks[from..to];
            let credited = window
                .iter()
                .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
                .map_or(organic, |c| c.2);
            let cell = campaign_daily.entry((credited, day)).or_default();
            cell.0 += 1;
            cell.1 += purchasing as i64;
            cell.2 += revenue;
            let mut assisting: Vec<u32> = window.iter().map(|c| c.2).collect();
            assisting.sort_unstable();
            assisting.dedup();
            for campaign in assisting {
                campaign_daily.entry((campaign, day)).or_default().3 += 1;
            }
            i = j;
        }
    }

    // ---- output
    std::fs::create_dir_all(out_dir.join("user_daily")).expect("create user_daily");
    std::fs::create_dir_all(out_dir.join("campaign_daily")).expect("create campaign_daily");
    fn put(buf: &mut Vec<u8>, s: &[u8]) {
        buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
        buf.extend_from_slice(s);
    }
    {
        let (mut ids, mut cats) = (Vec::<u8>::new(), Vec::<u8>::new());
        let (mut day, mut sessions, mut events, mut revenue) = (Vec::<i32>::new(), Vec::<i64>::new(), Vec::<i64>::new(), Vec::<i64>::new());
        let mut valid: Vec<bool> = Vec::new();
        for (&(person, d), row) in &user_daily {
            put(&mut ids, &users.names[person as usize]);
            day.push(d as i32);
            sessions.push(row.0);
            events.push(row.1);
            revenue.push(row.2);
            // highest sum; ties to the byte-wise smallest name (= smallest index)
            let top = row.3.iter().max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(&c, _)| c);
            valid.push(top.is_some());
            if let Some(c) = top {
                put(&mut cats, categories[c as usize]);
            }
        }
        let levels = parquet::def_levels(&valid);
        let columns = [
            Column { name: "user_id", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&ids] },
            Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![parquet::as_bytes(&day)] },
            Column { name: "sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&sessions)] },
            Column { name: "events", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&events)] },
            Column { name: "revenue_cents", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&revenue)] },
            Column { name: "top_category", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: true, payload: vec![&levels, &cats] },
        ];
        parquet::write(&out_dir.join("user_daily").join("part-000.parquet"), day.len(), &columns).expect("write user_daily");
    }
    {
        let (mut ids, mut channels) = (Vec::<u8>::new(), Vec::<u8>::new());
        let (mut day, mut sessions, mut purchasing, mut revenue, mut assisted) =
            (Vec::<i32>::new(), Vec::<i64>::new(), Vec::<i64>::new(), Vec::<i64>::new(), Vec::<i64>::new());
        for (&(campaign, d), cell) in &campaign_daily {
            put(&mut ids, campaign_ids[campaign as usize]);
            put(&mut channels, campaign_channels[campaign as usize]);
            day.push(d as i32);
            sessions.push(cell.0);
            purchasing.push(cell.1);
            revenue.push(cell.2);
            assisted.push(cell.3);
        }
        let columns = [
            Column { name: "campaign_id", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&ids] },
            Column { name: "channel", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&channels] },
            Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![parquet::as_bytes(&day)] },
            Column { name: "sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&sessions)] },
            Column { name: "purchasing_sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&purchasing)] },
            Column { name: "revenue_cents", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&revenue)] },
            Column { name: "assisted_sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&assisted)] },
        ];
        parquet::write(&out_dir.join("campaign_daily").join("part-000.parquet"), day.len(), &columns)
            .expect("write campaign_daily");
    }
}
