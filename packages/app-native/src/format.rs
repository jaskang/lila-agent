//! Pure formatting and encoding helpers.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tokens", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K tokens", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens} tokens")
    }
}

pub fn format_duration(ms: Option<f64>) -> Option<String> {
    let ms = ms?;
    if !ms.is_finite() || ms <= 0.0 {
        return None;
    }
    if ms < 1_000.0 {
        Some(format!("{}ms", ms.round() as u64))
    } else if ms < 60_000.0 {
        Some(format!("{:.1}s", ms / 1_000.0))
    } else {
        let mins = (ms / 60_000.0).floor() as u64;
        let secs = ((ms % 60_000.0) / 1_000.0).round() as u64;
        Some(format!("{mins}m{secs}s"))
    }
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn format_elapsed(started_at_ms: u64, now_ms: u64) -> String {
    let secs = now_ms.saturating_sub(started_at_ms) / 1_000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

pub fn format_short_time(iso: &str) -> String {
    // ISO string like "2026-11-23T09:59:28.123Z" -> "11-23 09:59"
    if iso.len() >= 16 && iso.as_bytes().get(10) == Some(&b'T') {
        format!("{} {}", &iso[5..10], &iso[11..16])
    } else {
        iso.to_owned()
    }
}

/// Parse an RFC-3339 / ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SS...`) into
/// Unix epoch seconds without external date crates.
fn parse_iso_utc_secs(iso: &str) -> Option<i64> {
    if iso.len() < 19 {
        return None;
    }
    let y: i64 = iso.get(0..4)?.parse().ok()?;
    let m: i64 = iso.get(5..7)?.parse().ok()?;
    let d: i64 = iso.get(8..10)?.parse().ok()?;
    let hh: i64 = iso.get(11..13)?.parse().ok()?;
    let mm: i64 = iso.get(14..16)?.parse().ok()?;
    let ss: i64 = iso.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's civil-from-days inverse algorithm.
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// Compact relative timestamp for sidebar rows (`刚刚`, `5m`, `2h`, `3d`, `11-23`).
pub fn format_relative_time(iso: &str, now_ms: u64) -> String {
    let Some(ts_secs) = parse_iso_utc_secs(iso) else {
        return String::new();
    };
    let now_secs = (now_ms / 1_000) as i64;
    let diff = now_secs.saturating_sub(ts_secs).max(0);
    if diff < 60 {
        "刚刚".to_owned()
    } else if diff < 3_600 {
        format!("{}m", diff / 60)
    } else if diff < 86_400 {
        format!("{}h", diff / 3_600)
    } else if diff < 86_400 * 30 {
        format!("{}d", diff / 86_400)
    } else if iso.len() >= 10 {
        iso[5..10].to_owned()
    } else {
        String::new()
    }
}

/// Shorten a user home directory prefix to `~` for calm monospace display.
pub fn shorten_home_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        if rest.is_empty() {
            return "~".to_owned();
        }
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    path.to_owned()
}

pub fn bytes_to_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 2 < bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        out.push(ALPHABET[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

/// Decode standard Base64 (ignoring ASCII whitespace) into raw bytes.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }

    let mut clean = Vec::with_capacity(input.len());
    for b in input.bytes() {
        if !b.is_ascii_whitespace() {
            clean.push(b);
        }
    }
    if clean.is_empty() {
        return Some(Vec::new());
    }

    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut i = 0;
    while i < clean.len() {
        let a = val(*clean.get(i)?)?;
        let b = val(*clean.get(i + 1)?)?;
        out.push((a << 2) | (b >> 4));

        if let Some(&c_byte) = clean.get(i + 2)
            && c_byte != b'='
        {
            let c = val(c_byte)?;
            out.push((b << 4) | (c >> 2));
            if let Some(&d_byte) = clean.get(i + 3)
                && d_byte != b'='
            {
                let d = val(d_byte)?;
                out.push((c << 6) | d);
            }
        }
        i += 4;
    }
    Some(out)
}

/// Percent-decode a URL path segment (e.g. `%20` or UTF-8 `%E4%B8%AD`) into a UTF-8 string.
pub fn percent_decode(input: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_owned())
}
