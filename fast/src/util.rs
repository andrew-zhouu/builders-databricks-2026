//! Calendar arithmetic, the exact (scalar) timestamp parser and the CSV dialect.

pub const DAY_MS: i64 = 86_400_000;
pub const GAP_MS: u32 = 1_800_000;
pub const REACH_MS: i64 = 7 * DAY_MS;

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        2 => {
            if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[inline]
fn two(b: &[u8], i: usize) -> Option<i64> {
    let (a, c) = (b[i].wrapping_sub(b'0'), b[i + 1].wrapping_sub(b'0'));
    if a > 9 || c > 9 {
        None
    } else {
        Some((a * 10 + c) as i64)
    }
}

/// Epoch milliseconds of a valid F1 / F2 / F3 timestamp. An F3 value too long for
/// 64 bits saturates: still a number, just outside every window.
pub fn parse_ts(b: &[u8]) -> Option<i64> {
    let n = b.len();
    if n == 0 {
        return None;
    }
    if b[0].is_ascii_digit() && (n < 5 || b[4] != b'-') {
        let mut v: i64 = 0;
        for &c in b {
            let d = c.wrapping_sub(b'0');
            if d > 9 {
                return None;
            }
            v = v.saturating_mul(10).saturating_add(d as i64);
        }
        return Some(v);
    }
    if n != 20 && n != 25 {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y = two(b, 0)? * 100 + two(b, 2)?;
    let mo = two(b, 5)?;
    let d = two(b, 8)?;
    let h = two(b, 11)?;
    let mi = two(b, 14)?;
    let s = two(b, 17)?;
    if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let ms = (days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + s) * 1_000;
    if n == 20 {
        return if b[19] == b'Z' { Some(ms) } else { None };
    }
    let sign = b[19];
    if (sign != b'+' && sign != b'-') || b[22] != b':' {
        return None;
    }
    let oh = two(b, 20)?;
    let om = two(b, 23)?;
    if oh > 23 || om > 59 {
        return None;
    }
    let off = (oh * 60 + om) * 60_000;
    Some(if sign == b'+' { ms - off } else { ms + off })
}

/// A purchase price: ASCII digits only, value 1 to 10,000,000 (leading zeros allowed).
pub fn parse_price(b: &[u8]) -> Option<u32> {
    if b.is_empty() {
        return None;
    }
    let mut v: u64 = 0;
    for &c in b {
        let d = c.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        v = (v * 10 + d as u64).min(1_000_000_000);
    }
    if (1..=10_000_000).contains(&v) {
        Some(v as u32)
    } else {
        None
    }
}

/// Split one line by the rules' dialect into exactly `N` fields (quotes removed).
/// `None` for a malformed line: wrong field count, or a quote anywhere except
/// around a whole field.
#[inline]
pub fn split<const N: usize>(line: &[u8]) -> Option<[&[u8]; N]> {
    let mut out: [&[u8]; N] = [&line[0..0]; N];
    let n = line.len();
    let mut k = 0;
    let mut s = 0;
    loop {
        let mut e = s;
        let mut q = 0u32;
        let mut first_q = usize::MAX;
        let mut last_q = 0;
        while e < n && line[e] != b',' {
            if line[e] == b'"' {
                if q == 0 {
                    first_q = e;
                }
                last_q = e;
                q += 1;
            }
            e += 1;
        }
        if k == N {
            return None;
        }
        out[k] = if q == 0 {
            &line[s..e]
        } else if q == 2 && first_q == s && last_q == e - 1 {
            &line[s + 1..e - 1]
        } else {
            return None;
        };
        k += 1;
        if e == n {
            break;
        }
        s = e + 1;
    }
    if k == N {
        Some(out)
    } else {
        None
    }
}

#[inline]
pub fn find_newline(data: &[u8], from: usize) -> Option<usize> {
    data[from..].iter().position(|&c| c == b'\n').map(|i| from + i)
}

/// Calls `f` for every data line that starts inside `[start, end)`. The first
/// line of a file is its header and is never passed on. Only `\n` ends a line;
/// a final line without one still counts.
#[inline]
pub fn for_each_line<'a>(data: &'a [u8], start: usize, end: usize, mut f: impl FnMut(&'a [u8])) {
    let len = data.len();
    let mut pos = match find_newline(data, start.saturating_sub(1).min(len)) {
        Some(i) => i + 1,
        None => return,
    };
    while pos < end {
        let e = find_newline(data, pos).unwrap_or(len);
        f(&data[pos..e]);
        pos = e + 1;
    }
}
