//! The read endpoints behind admission (spec §6, AC-6): addons, LedgerLink
//! SavedVariables raw + parsed, allowlisted addon SavedVariables raw
//! (`addon_sv`), dev snapshots (`snapshot_routes`), logs, events, collectors. `/health` stays in server.rs;
//! the probe endpoints arrive with C4.

use super::events::EventLog;
use super::health::discover_flavors;
use super::logbuf::{is_level, parse_utc, LogBuffer};
use super::paging::{decode_cursor, encode_cursor, invalid, page, wrap_next, Query};
use super::server::Snapshot;
use super::{addon_sv, addons, snapshot_routes, sv};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// Builds the `collectors` body (`$defs/collectors`) from the runtime.
pub type CollectorsFn = Arc<dyn Fn() -> Value + Send + Sync>;

type Res = Result<Value, (u16, Value)>;

fn bad(field: &str, msg: &str) -> (u16, Value) {
    (400, invalid(field, msg))
}

fn not_found() -> (u16, Value) {
    (404, json!({ "error": "not_found" }))
}

/// GET `path` → `(status, body)`, or `None` when no read endpoint has it.
pub fn get(
    snap: &Snapshot,
    path: &str,
    q: &Query,
    collectors: Option<&CollectorsFn>,
    logs: &LogBuffer,
    events: &EventLog,
) -> Option<(u16, Value)> {
    let res = match path {
        "/dev/v1/addons" => addons_ep(snap, q),
        p if addon_sv::segment(p).is_some() => addon_sv_ep(snap, p, q),
        "/dev/v1/ledgerlink/sv" => sv_raw_ep(snap, q, sv::SV_FILE),
        "/dev/v1/ledgerlink/sv/parsed" => sv_parsed_ep(snap, q),
        "/dev/v1/ledgerlink/snapshot" => {
            flavor(snap, q).and_then(|f| snapshot_routes::page_ep(snap, &f, q))
        }
        "/dev/v1/ledgerlink/snapshot/history" => {
            flavor(snap, q).and_then(|f| snapshot_routes::history_ep(snap, &f, q))
        }
        "/dev/v1/logs" => logs_ep(logs, q),
        "/dev/v1/events" => events
            .page(q.get("since"), q.get("cursor"))
            .map_err(|b| (400, b)),
        "/dev/v1/collectors" => Ok(collectors.map_or_else(|| json!({ "collectors": [] }), |f| f())),
        _ => return None,
    };
    Some(match res {
        Ok(body) => (200, body),
        Err(e) => e,
    })
}

/// `?flavor=` matched against the discovered folders (never used as a path
/// before that); optional when exactly one flavor exists.
fn flavor(snap: &Snapshot, q: &Query) -> Result<String, (u16, Value)> {
    let dir = snap.wow_dir.trim();
    let found = if dir.is_empty() {
        Vec::new()
    } else {
        discover_flavors(Path::new(dir))
    };
    match q.get("flavor") {
        Some(f) if found.contains(f) => Ok(f.clone()),
        Some(_) => Err(bad("flavor", "Unknown flavor (see health.flavors)")),
        None if found.len() == 1 => found.first().cloned().ok_or_else(not_found),
        None => Err(bad("flavor", "Pass ?flavor= (one of health.flavors)")),
    }
}

fn cursor(q: &Query) -> Result<usize, (u16, Value)> {
    decode_cursor(q.get("cursor")).map_err(|b| (400, b))
}

fn addons_ep(snap: &Snapshot, q: &Query) -> Res {
    let flavor = flavor(snap, q)?;
    let start = cursor(q)?;
    let rows = addons::addons(Path::new(snap.wow_dir.trim()), &flavor);
    Ok(page(
        &rows,
        start,
        200,
        |items, next| json!({ "flavor": flavor, "addons": items, "next": next }),
    ))
}

/// `/dev/v1/addons/<folder>/sv`: an unlisted folder is 404 before any query
/// check, so the route reveals nothing about folders outside the allowlist.
fn addon_sv_ep(snap: &Snapshot, path: &str, q: &Query) -> Res {
    let file = addon_sv::segment(path)
        .and_then(addon_sv::sv_file)
        .ok_or_else(not_found)?;
    sv_raw_ep(snap, q, file)
}

/// `sv-raw` for `SavedVariables/<sv_file>` (a constant, never caller input).
fn sv_raw_ep(snap: &Snapshot, q: &Query, sv_file: &str) -> Res {
    let flavor = flavor(snap, q)?;
    let scope = q.get("scope").map_or("account", String::as_str);
    if scope != "account" && scope != "character" {
        return Err(bad("scope", "scope is account or character"));
    }
    let cursor = q.get("cursor").map(String::as_str);
    let wow = Path::new(snap.wow_dir.trim());
    sv::raw_named(wow, &flavor, sv_file, scope, cursor).map_err(|e| match e {
        sv::RawError::Missing => not_found(),
        sv::RawError::BadCursor => bad("cursor", "Unknown cursor"),
        sv::RawError::Changed => bad(
            "cursor",
            "The file changed since this cursor; restart from the first page",
        ),
    })
}

fn sv_parsed_ep(snap: &Snapshot, q: &Query) -> Res {
    let flavor = flavor(snap, q)?;
    let start = cursor(q)?;
    let rows: Vec<Value> = sv::sv_files(Path::new(snap.wow_dir.trim()), &flavor)
        .iter()
        .flat_map(sv::parsed_rows)
        .collect();
    Ok(page(
        &rows,
        start,
        50,
        |items, next| json!({ "flavor": flavor, "files": items, "next": next }),
    ))
}

/// Logs page by sequence number, so a cursor survives the ring rotating.
fn logs_ep(logs: &LogBuffer, q: &Query) -> Res {
    let from = cursor(q)?;
    let level = q.get("level").map(String::as_str);
    if level.is_some_and(|l| !is_level(l)) {
        return Err(bad("level", "level is error, warn, info, debug or trace"));
    }
    let since = match q.get("since") {
        None => None,
        Some(s) => match parse_utc(s) {
            Some(t) if s.len() <= 24 => Some(t),
            _ => return Err(bad("since", "since is an RFC 3339 UTC time")),
        },
    };
    let selected = logs.select(from, level, since);
    let lines: Vec<Value> = selected.iter().map(|(_, v)| v.clone()).collect();
    let mut body = page(
        &lines,
        0,
        500,
        |items, next| json!({ "lines": items, "next": next }),
    );
    // `page` cursors are indexes into this selection; re-key them by seq.
    let idx = decode_cursor(body["next"].as_str().map(str::to_owned).as_ref()).unwrap_or(0);
    if !body["next"].is_null() {
        let seq = selected.get(idx).map_or(from, |(s, _)| *s);
        body["next"] = wrap_next(Some(encode_cursor(seq)));
    }
    Ok(body)
}
