//! `probe-list` and `probe-detail` bodies (ROK-2191). Both are paged with
//! route-bound cursors: `l<probeId>` (list: the last row returned) and
//! `d<probeId>:<item>` (detail: that probe's next result item), base64url —
//! a cursor from another route or another probe is a 400 at `cursor`.

use super::paging::{json_len, page, Query, PAGE_MAX_BYTES};
use super::probe_record::{ProbeRecord, ProbeStatus};
use super::probe_result_page::{assemble_page, page_items};
use super::probe_routes::{issue, Res};
use super::probe_store::ProbeStore;
use base64::Engine;
use serde_json::{json, Value};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const LIST_MAX: usize = 100;
/// The longest cursor the schema allows; pages reserve room for it.
const WORST_NEXT: usize = 128;
const STATUS_MSG: &str =
    r#"Invalid option: expected one of "queued"|"answered"|"expired"|"rejected"|"cancelled""#;

fn bad_cursor() -> Res {
    issue(json!(["cursor"]), "custom", "Unknown cursor")
}

fn encode(tag: char, rest: &str) -> String {
    B64.encode(format!("{tag}{rest}"))
}

fn decode(raw: &str, tag: char) -> Option<String> {
    let s = String::from_utf8(B64.decode(raw).ok()?).ok()?;
    s.strip_prefix(tag).map(str::to_owned)
}

/// GET /dev/v1/probes — newest first, ≤ 100 rows and ≤ 16 KiB a page.
pub fn list(store: &ProbeStore, q: &Query) -> Result<Value, Res> {
    let status = match q.get("status") {
        None => None,
        Some(s) => Some(
            ProbeStatus::parse(s)
                .ok_or_else(|| issue(json!(["status"]), "invalid_value", STATUS_MSG))?,
        ),
    };
    let mut recs = store.list(status);
    recs.reverse();
    let start = match q.get("cursor") {
        None => 0,
        Some(raw) => {
            let after = decode(raw, 'l').ok_or_else(bad_cursor)?;
            let at = recs.iter().position(|r| r.probe_id == after);
            at.map(|i| i + 1).ok_or_else(bad_cursor)?
        }
    };
    let rows: Vec<Value> = recs.iter().map(ProbeRecord::summary_json).collect();
    let wrap = |items, next| json!({ "probes": items, "next": next });
    let mut body = page(&rows, start, LIST_MAX, wrap);
    if !body["next"].is_null() {
        let last = body["probes"].as_array().and_then(|a| a.last());
        let id = last.and_then(|r| r["probeId"].as_str()).unwrap_or_default();
        body["next"] = Value::String(encode('l', id));
    }
    Ok(body)
}

/// Everything but `result` and `next`: repeated on every page.
fn base(rec: &ProbeRecord) -> Value {
    let mut v = rec.summary_json();
    v["request"] = rec.request.clone();
    if let Some(e) = &rec.error {
        v["error"] = Value::String(e.clone());
    }
    v
}

/// GET /dev/v1/probes/:id — the result paged by item, each page ≤ 16 KiB.
pub fn detail(rec: &ProbeRecord, cursor: Option<&String>) -> Result<Value, Res> {
    let id = &rec.probe_id;
    let start = match cursor {
        None => None,
        Some(raw) => {
            let rest = decode(raw, 'd').ok_or_else(bad_cursor)?;
            let (cid, n) = rest.split_once(':').ok_or_else(bad_cursor)?;
            let n: usize = n.parse().map_err(|_| bad_cursor())?;
            (cid == id).then_some(n).map(Some).ok_or_else(bad_cursor)?
        }
    };
    let mut body = base(rec);
    let Some(result) = &rec.result else {
        if start.is_some() {
            return Err(bad_cursor());
        }
        body["next"] = Value::Null;
        return Ok(body);
    };
    let items = page_items(result);
    let start = start.unwrap_or(0);
    if start > 0 && start >= items.len() {
        return Err(bad_cursor());
    }
    let mut probe = body.clone();
    probe["result"] = json!({});
    probe["next"] = Value::String("A".repeat(WORST_NEXT));
    // `{}` is replaced by the result object, so its 2 bytes are free.
    let budget = PAGE_MAX_BYTES.saturating_sub(json_len(&probe) - 2);
    let (slice, next) = assemble_page(result, &items, start, budget);
    body["result"] = slice;
    body["next"] = next.map_or(Value::Null, |n| {
        Value::String(encode('d', &format!("{id}:{n}")))
    });
    Ok(body)
}
