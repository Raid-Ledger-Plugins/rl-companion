//! `/dev/v1/probes` routes (ROK-2191, CONTRACT "Endpoints"): POST queues a
//! probe (202 `probe-queued`), GET lists or reads one (`probe-list`,
//! `probe-detail`, answers ingested first), DELETE cancels (`probe-cancelled`).
//! Only POST answers 409 `probes_disabled` while probes are off — the
//! contract lists it for POST alone; stored probes stay readable/cancellable.

use super::paging::{parse_query, Query};
use super::probe_detail::{detail, list};
use super::probe_ingest::refresh;
use super::probe_queue::QUEUE_MAX;
use super::probe_request::{error_body, validate};
use super::probe_store::{ProbeStore, ProbeStoreError};
use super::request_body::ReqBody;
use super::server::Snapshot;
use super::write_guard::WriteGuardError;
use hyper::Method;
use serde_json::{json, Value};
use std::path::Path;

pub const PROBES_PATH: &str = "/dev/v1/probes";
const ANSWERS_ON: &str = "next UI load, then next save";

pub type Res = (u16, Value);

pub fn not_found() -> Res {
    (404, json!({ "error": "not_found" }))
}

fn internal() -> Res {
    (500, json!({ "error": "internal" }))
}

/// 400 `invalid_request` with one issue.
pub fn issue(path: Value, code: &str, message: &str) -> Res {
    let i = json!({ "path": path, "code": code, "message": message });
    (400, json!({ "error": "invalid_request", "issues": [i] }))
}

/// Route a probe request; `None` when `path` is not a probe route.
pub fn dispatch(
    snap: &Snapshot,
    method: &Method,
    path: &str,
    query: Option<&str>,
    body: &ReqBody,
    now: i64,
) -> Option<Res> {
    let id = match path.strip_prefix(PROBES_PATH)? {
        "" => None,
        rest => Some(rest.strip_prefix('/')?),
    };
    let q = parse_query(query);
    Some(match (method, id) {
        (&Method::POST, None) => post(snap, body, now),
        (&Method::GET, None) => with_store(snap, now, |s| Ok((200, list(s, &q)?))),
        (&Method::GET, Some(id)) => with_store(snap, now, |s| get_one(s, id, &q)),
        (&Method::DELETE, Some(id)) => with_store(snap, now, |s| Ok(cancel(s, snap, id, now))),
        _ => not_found(),
    })
}

/// Run `f` on the store after ingesting answers. No store: 409 while
/// probes are off, else 500.
fn with_store(snap: &Snapshot, now: i64, f: impl FnOnce(&ProbeStore) -> Result<Res, Res>) -> Res {
    let Some(store) = snap.probe_store.as_deref() else {
        return if snap.probes_enabled {
            internal()
        } else {
            disabled()
        };
    };
    refresh(store, &snap.wow_dir, now);
    f(store).unwrap_or_else(|e| e)
}

fn disabled() -> Res {
    (409, json!({ "error": "probes_disabled" }))
}

fn get_one(store: &ProbeStore, id: &str, q: &Query) -> Result<Res, Res> {
    if !rl_collectors::is_probe_id(id) {
        return Err(not_found());
    }
    let rec = store.get(id).ok_or_else(not_found)?;
    Ok((200, detail(&rec, q.get("cursor"))?))
}

fn post(snap: &Snapshot, body: &ReqBody, now: i64) -> Res {
    if !snap.probes_enabled {
        return disabled();
    }
    let json = match parse_body(body) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let req = match validate(&json) {
        Ok(r) => r,
        Err(issues) => return (400, error_body(&issues)),
    };
    let Some(store) = snap.probe_store.as_deref() else {
        return internal();
    };
    match store.submit(Path::new(snap.wow_dir.trim()), &req, now) {
        Ok(rec) => (
            202,
            json!({ "probeId": rec.probe_id, "status": "queued",
                "flavor": rec.flavor, "answersOn": ANSWERS_ON }),
        ),
        Err(e) => store_error(e),
    }
}

fn parse_body(body: &ReqBody) -> Result<Value, Res> {
    let not_json = || {
        issue(
            json!([]),
            "invalid_type",
            "Invalid input: expected a JSON body",
        )
    };
    match body {
        ReqBody::TooBig => Err(issue(json!([]), "too_big", "Request body is over 8 KiB")),
        ReqBody::None => Err(not_json()),
        ReqBody::Bytes(b) => serde_json::from_slice(b).map_err(|_| not_json()),
    }
}

fn cancel(store: &ProbeStore, snap: &Snapshot, id: &str, now: i64) -> Res {
    if !rl_collectors::is_probe_id(id) {
        return not_found();
    }
    let done = || (200, json!({ "probeId": id, "status": "cancelled" }));
    match store.cancel(Path::new(snap.wow_dir.trim()), id, now) {
        Ok(_) | Err(ProbeStoreError::NotQueued(_, "cancelled")) => done(),
        Err(e) => store_error(e),
    }
}

/// The HTTP answer for a store failure (never the underlying IO text).
fn store_error(e: ProbeStoreError) -> Res {
    match e {
        ProbeStoreError::Flavor(f) => issue(json!(["flavor"]), "custom", &f.to_string()),
        ProbeStoreError::Write(WriteGuardError::NotInstalled(_)) => issue(
            json!(["flavor"]),
            "custom",
            "LedgerLink is not installed in this flavor",
        ),
        ProbeStoreError::QueueFull(_) => issue(
            json!([]),
            "custom",
            &format!("the probe queue for this flavor is full ({QUEUE_MAX})"),
        ),
        ProbeStoreError::NotFound(_) => not_found(),
        ProbeStoreError::NotQueued(_, s) => issue(json!([]), "custom", &format!("probe is {s}")),
        ProbeStoreError::Write(_) | ProbeStoreError::Persist(_) => internal(),
    }
}
