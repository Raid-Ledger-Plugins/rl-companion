//! `GET /dev/v1/ledgerlink/snapshot` and `/snapshot/history` (ROK-2072 §4,
//! AC-10): cached LedgerLink dev snapshots, every body ≤ 16 KiB. Read-tier
//! (GET only). Also the 30 s poll and the read-through that feed the cache.

use super::health::discover_flavors;
use super::paging::{invalid, json_len, page, Query, PAGE_MAX_BYTES};
use super::server::{ServerCtx, Snapshot};
use super::snapshot_cache::{Row, SnapshotCache, POLL_INTERVAL};
use super::snapshot_validate::head;
use super::sv;
use base64::Engine;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

type Res = Result<Value, (u16, Value)>;

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
/// Page items are `calls` → `apis` → `sections`, in that order.
const ARRAYS: [&str; 3] = ["calls", "apis", "sections"];
const HISTORY_MAX: usize = 100;
/// The longest cursor the schema allows; pages reserve room for it.
const WORST_NEXT: usize = 128;

fn bad(field: &str, msg: &str) -> (u16, Value) {
    (400, invalid(field, msg))
}

fn not_found() -> (u16, Value) {
    (404, json!({ "error": "not_found" }))
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Ingest every account-scope `LedgerLink.lua` of `flavor` that changed.
pub fn refresh(cache: &SnapshotCache, wow_dir: &str, flavor: &str, now: i64) {
    let dir = wow_dir.trim();
    if dir.is_empty() {
        return;
    }
    for f in sv::sv_files(Path::new(dir), flavor) {
        if f.scope == "account" {
            cache.ingest_file(flavor, &f.abs, now);
        }
    }
}

/// One poll tick over every discovered flavor.
pub fn poll_once(cache: &SnapshotCache, wow_dir: &str, now: i64) {
    let dir = wow_dir.trim();
    if dir.is_empty() {
        return;
    }
    for flavor in discover_flavors(Path::new(dir)) {
        refresh(cache, dir, &flavor, now);
    }
}

/// D9: runs while the listener does (spawned in `listen`, aborted in
/// `shut`). The first tick fires at once and prunes (listener start).
pub async fn poll_loop(ctx: Arc<ServerCtx>) {
    let mut tick = tokio::time::interval(POLL_INTERVAL);
    let mut first = true;
    loop {
        tick.tick().await;
        let snap = ctx.snapshot();
        let Some(cache) = snap.snapshot_cache else {
            continue;
        };
        let prune = std::mem::replace(&mut first, false);
        let wow = snap.wow_dir;
        let _ = tokio::task::spawn_blocking(move || {
            let now = now_secs();
            if prune {
                cache.prune(now);
            }
            poll_once(&cache, &wow, now);
        })
        .await;
    }
}

/// The cache, refreshed from `flavor`'s SavedVariables (read-through).
fn cache_for<'a>(snap: &'a Snapshot, flavor: &str) -> Result<&'a SnapshotCache, (u16, Value)> {
    let cache = snap.snapshot_cache.as_deref().ok_or_else(not_found)?;
    refresh(cache, &snap.wow_dir, flavor, now_secs());
    Ok(cache)
}

/// `?character=`: a cached `characterKey`, optional when only one is cached.
fn character(cache: &SnapshotCache, flavor: &str, q: &Query) -> Result<String, (u16, Value)> {
    let keys = cache.characters(flavor);
    match q.get("character") {
        Some(c) if keys.contains(c) => Ok(c.clone()),
        Some(_) => Err(bad("character", "Unknown character (see /snapshot/history)")),
        None => match keys.as_slice() {
            [] => Err(not_found()),
            [only] => Ok(only.clone()),
            _ => Err(bad("character", "Pass ?character= (see /snapshot/history)")),
        },
    }
}

fn decode(raw: &str, tag: char) -> Option<String> {
    let s = String::from_utf8(B64.decode(raw).ok()?).ok()?;
    s.strip_prefix(tag).map(str::to_owned)
}

/// The snapshot cursor: `(snapshotId, item offset)`.
fn snap_cursor(raw: &str) -> Option<(String, usize)> {
    let s = decode(raw, 's')?;
    let (id, off) = s.split_once(':')?;
    Some((id.to_owned(), off.parse().ok()?))
}

/// GET /dev/v1/ledgerlink/snapshot — `ledgerlink-snapshot-page`.
pub fn page_ep(snap: &Snapshot, flavor: &str, q: &Query) -> Res {
    let cache = cache_for(snap, flavor)?;
    let key = character(cache, flavor, q)?;
    let rows = cache.rows(flavor, &key);
    let (want, start) = match q.get("cursor") {
        None => (q.get("at").cloned(), 0),
        Some(raw) => {
            let (id, off) = snap_cursor(raw).ok_or_else(|| bad("cursor", "Unknown cursor"))?;
            let known = rows.iter().any(|r| r.snapshot_id == id);
            if !known || q.get("at").is_some_and(|a| *a != id) {
                let msg = "Unknown cursor (snapshot pruned?); restart from /snapshot/history";
                return Err(bad("cursor", msg));
            }
            (Some(id), off)
        }
    };
    let idx = match want {
        None => rows.len().checked_sub(1),
        Some(id) => rows.iter().position(|r| r.snapshot_id == id),
    };
    let row = idx.and_then(|i| rows.get(i)).ok_or_else(not_found)?;
    let latest = idx.is_some_and(|i| i + 1 == rows.len());
    Ok(page_body(flavor, &key, row, latest, start))
}

/// One page from item `start`: the full head plus as many items as fit.
pub fn page_body(flavor: &str, key: &str, row: &Row, latest: bool, start: usize) -> Value {
    let items: Vec<(&str, &Value)> = ARRAYS
        .iter()
        .flat_map(|k| {
            let list = row.snapshot.get(*k).and_then(Value::as_array);
            list.into_iter().flatten().map(move |v| (*k, v))
        })
        .collect();
    let start = start.min(items.len());
    let build = |end: usize, next: Value| {
        let mut s = head(&row.snapshot);
        for k in ARRAYS {
            s[k] = json!([]);
        }
        for (k, v) in items.get(start..end).unwrap_or_default() {
            if let Some(a) = s[*k].as_array_mut() {
                a.push((*v).clone());
            }
        }
        json!({
            "flavor": flavor, "characterKey": key, "snapshotId": row.snapshot_id,
            "digest": row.digest, "receivedAt": row.received_at,
            "lastCapturedAt": row.last_captured_at, "seenCount": row.seen_count,
            "latest": latest, "snapshot": s, "next": next,
        })
    };
    let worst = Value::String("A".repeat(WORST_NEXT));
    let mut end = start;
    while end < items.len() && json_len(&build(end + 1, worst.clone())) <= PAGE_MAX_BYTES {
        end += 1;
    }
    if end == start && start < items.len() {
        end += 1;
    }
    let next = if end < items.len() {
        Value::String(B64.encode(format!("s{}:{end}", row.snapshot_id)))
    } else {
        Value::Null
    };
    build(end, next)
}

fn history_entry(flavor: &str, key: &str, r: &Row, latest: bool) -> Value {
    let s = &r.snapshot;
    let missing = s["apis"].as_array().map_or(0, |a| {
        a.iter().filter(|x| x["type"] == "nil").count()
    });
    let active: Vec<Value> = s["ruleset"]["rules"].as_array().map_or_else(Vec::new, |a| {
        a.iter()
            .filter(|x| x["active"] == true)
            .map(|x| x["name"].clone())
            .collect()
    });
    json!({
        "snapshotId": r.snapshot_id, "flavor": flavor, "characterKey": key,
        "digest": r.digest, "capturedAt": r.captured_at,
        "lastCapturedAt": r.last_captured_at, "receivedAt": r.received_at,
        "seenCount": r.seen_count, "latest": latest,
        "addonVersion": s["addonVersion"], "clientVersion": s["client"]["version"],
        "clientBuild": s["client"]["build"], "interface": s["client"]["interface"],
        "missingApis": missing, "activeRules": active, "truncated": s["truncated"],
    })
}

/// GET /dev/v1/ledgerlink/snapshot/history — newest first; the cursor is the
/// last returned `snapshotId` (the next page is the rows older than it).
pub fn history_ep(snap: &Snapshot, flavor: &str, q: &Query) -> Res {
    let cache = cache_for(snap, flavor)?;
    let keys = match q.get("character") {
        None => cache.characters(flavor),
        Some(c) if cache.characters(flavor).contains(c) => vec![c.clone()],
        Some(_) => return Err(bad("character", "Unknown character")),
    };
    let mut entries: Vec<Value> = Vec::new();
    for key in &keys {
        let rows = cache.rows(flavor, key);
        for (i, r) in rows.iter().enumerate() {
            entries.push(history_entry(flavor, key, r, i + 1 == rows.len()));
        }
    }
    if entries.is_empty() {
        return Err(not_found());
    }
    let id = |e: &Value| e["snapshotId"].as_str().unwrap_or_default().to_owned();
    entries.sort_by_key(|e| std::cmp::Reverse(id(e)));
    let start = match q.get("cursor") {
        None => 0,
        Some(raw) => {
            let after = decode(raw, 'h').ok_or_else(|| bad("cursor", "Unknown cursor"))?;
            entries.iter().position(|e| id(e) < after).unwrap_or(entries.len())
        }
    };
    let mut body = page(&entries, start, HISTORY_MAX, |items, next| {
        json!({ "entries": items, "next": next })
    });
    if !body["next"].is_null() {
        let last = body["entries"].as_array().and_then(|a| a.last()).map(id);
        body["next"] = Value::String(B64.encode(format!("h{}", last.unwrap_or_default())));
    }
    Ok(body)
}
