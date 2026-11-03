//! Paging + query helpers shared by every `/dev/v1/*` body (AC-6): each
//! serialised body is ≤ 16 KiB; a list that would not fit carries an opaque
//! `next` cursor (`^[A-Za-z0-9_-]{1,128}$`, `null` on the last page).

use base64::Engine;
use serde_json::{json, Value};
use std::collections::HashMap;

/// `DEV_BRIDGE_PAGE_MAX_BYTES`: UTF-8 bytes of the JSON serialisation.
pub const PAGE_MAX_BYTES: usize = 16 * 1024;
/// The longest cursor the schema allows; pages reserve room for it.
const WORST_NEXT: usize = 128;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub type Query = HashMap<String, String>;

/// Decoded `?a=b&c=d` (first value wins).
pub fn parse_query(raw: Option<&str>) -> Query {
    let mut out = Query::new();
    for (k, v) in url::form_urlencoded::parse(raw.unwrap_or("").as_bytes()) {
        out.entry(k.into_owned()).or_insert_with(|| v.into_owned());
    }
    out
}

pub fn encode_cursor(offset: usize) -> String {
    B64.encode(format!("o{offset}"))
}

/// `?cursor=` → offset (0 when absent); an unknown cursor is a 400 body.
pub fn decode_cursor(raw: Option<&String>) -> Result<usize, Value> {
    let Some(raw) = raw else { return Ok(0) };
    B64.decode(raw)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.strip_prefix('o').and_then(|n| n.parse().ok()))
        .ok_or_else(|| invalid("cursor", "Unknown cursor"))
}

/// A 400 `error` body with one issue at `field`.
pub fn invalid(field: &str, message: &str) -> Value {
    json!({
        "error": "invalid_request",
        "issues": [{ "path": [field], "code": "custom", "message": message }],
    })
}

pub fn json_len(v: &Value) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

fn next_value(next: Option<String>) -> Value {
    next.map_or(Value::Null, Value::String)
}

/// One page of `items` from `start`: as many as fit in 16 KiB and
/// `max_items`. `wrap(page, next)` builds the body around the slice.
/// Builders cap each item well under a page, so one item always fits.
pub fn page(
    items: &[Value],
    start: usize,
    max_items: usize,
    wrap: impl Fn(Vec<Value>, Value) -> Value,
) -> Value {
    let start = start.min(items.len());
    let mut used = json_len(&wrap(Vec::new(), Value::String("A".repeat(WORST_NEXT))));
    let mut end = start;
    while end < items.len() && end - start < max_items {
        let add = json_len(&items[end]) + usize::from(end > start);
        if used + add > PAGE_MAX_BYTES {
            break;
        }
        used += add;
        end += 1;
    }
    if end == start && start < items.len() {
        end += 1;
    }
    let next = (end < items.len()).then(|| encode_cursor(end));
    wrap(items[start..end].to_vec(), next_value(next))
}

/// Cut `text` from byte `start` (a char boundary) so the slice serialised as
/// a JSON string is ≤ `budget` bytes and ≤ `max_chars` chars. Returns the
/// slice and the byte offset where the next page starts.
pub fn text_slice(text: &str, start: usize, budget: usize, max_chars: usize) -> (&str, usize) {
    let mut used = 0usize;
    let mut end = start;
    let mut buf = [0u8; 4];
    for (n, c) in text[start..].chars().enumerate() {
        if n >= max_chars {
            break;
        }
        let cost = escaped_len(c, &mut buf);
        if used + cost > budget {
            break;
        }
        used += cost;
        end += c.len_utf8();
    }
    (&text[start..end], end)
}

/// Bytes `c` takes inside a JSON string (serde_json's escaping).
fn escaped_len(c: char, buf: &mut [u8; 4]) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.encode_utf8(buf).len(),
    }
}

/// `s` cut to at most `max` chars.
pub fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The page that a body with `next` would need to fit (for tests).
pub fn wrap_next(next: Option<String>) -> Value {
    next_value(next)
}
