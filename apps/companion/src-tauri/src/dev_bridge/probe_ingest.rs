//! Probe answer ingest (ROK-2191): read each flavor's account-scope
//! `LedgerLink.lua` (`LedgerLinkDB.dev.probes`) and settle every queued probe
//! LedgerLink answered, then expire the rest that are past their ttl (answers
//! first, so one stored in time but read late is not thrown away) — `answered` when the result validates and
//! keeps name-bearing calls presence-only, else `rejected`. Runs before every
//! probe route and on the 30 s poll. Result values are never logged.

use super::probe_record::{ProbeRecord, ProbeStatus};
use super::probe_result::validate_result;
use super::probe_store::ProbeStore;
use super::snapshot_validate::{LUA_TYPES, NAME_BEARING_FNS};
use super::sv;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// Apply every stored answer to its queued probe, then expire.
pub fn refresh(store: &ProbeStore, wow_dir: &str, now: i64) {
    let dir = wow_dir.trim();
    if dir.is_empty() {
        return;
    }
    let wow = Path::new(dir);
    let queued = store.list(Some(ProbeStatus::Queued));
    let mut flavors: Vec<&str> = queued.iter().map(|r| r.flavor.as_str()).collect();
    flavors.sort_unstable();
    flavors.dedup();
    for flavor in flavors {
        let answers = answers(wow, flavor);
        for rec in queued.iter().filter(|r| r.flavor == flavor) {
            if let Some(raw) = answers.get(&rec.probe_id) {
                let _ = store.apply_answer(wow, &rec.probe_id, outcome(rec, raw), now);
            }
        }
    }
    store.expire_due(wow, now);
}

/// The stored result → `Ok(result)` or `Err(why it is rejected)`.
pub fn outcome(rec: &ProbeRecord, raw: &str) -> Result<Value, String> {
    let v = validate_result(raw, &rec.probe_id)?;
    check_name_bearing(&rec.request, &v)?;
    Ok(v)
}

/// Probe id → raw result JSON from `flavor`'s account-scope files (the
/// symlink-safe listing `sv.rs` serves from; first file wins on a clash).
fn answers(wow: &Path, flavor: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for f in sv::sv_files(wow, flavor) {
        if f.scope != "account" {
            continue;
        }
        let Ok(bytes) = std::fs::read(&f.abs) else {
            continue;
        };
        if let Ok(d) = rl_collectors::parse_ledgerlink_dev_probes(&bytes) {
            for (id, raw) in d.results {
                out.entry(id).or_insert(raw);
            }
        }
    }
    out
}

/// A name-bearing call (and each iteration) may carry no values, or exactly
/// one string that is a Lua type name (presence only).
pub fn check_name_bearing(request: &Value, result: &Value) -> Result<(), String> {
    let calls = request.get("calls").and_then(Value::as_array);
    let results = result.get("calls").and_then(Value::as_array);
    for call in calls.into_iter().flatten() {
        let named = call.get("fn").and_then(Value::as_str);
        if !named.is_some_and(is_name_bearing) {
            continue;
        }
        let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
        let found = results
            .into_iter()
            .flatten()
            .filter(|r| r.get("id").and_then(Value::as_str) == Some(id));
        for r in found {
            let its = r.get("iterations").and_then(Value::as_array);
            let ok = presence_only(r) && its.into_iter().flatten().all(presence_only);
            if !ok {
                return Err(format!("name-bearing call {id} returned values"));
            }
        }
    }
    Ok(())
}

/// By the path's LAST segment, so `_G.UnitName` (an alias LedgerLink may
/// resolve to the same function) is held to presence-only too.
fn is_name_bearing(path: &str) -> bool {
    let leaf = path.rsplit('.').next().unwrap_or(path);
    NAME_BEARING_FNS.contains(&leaf)
}

fn presence_only(item: &Value) -> bool {
    match item.get("values") {
        None => true,
        Some(Value::Array(a)) => match a.as_slice() {
            [] => true,
            [Value::String(s)] => LUA_TYPES.contains(&s.as_str()),
            _ => false,
        },
        Some(_) => false,
    }
}
