//! `GET /dev/v1/collectors` (`$defs/collectors`): the same data the window's
//! `get_collector_status` shows, reduced to the contract's fields.

use crate::runtime::CollectorStatusDto;
use serde_json::{json, Value};

const MAX_COLLECTORS: usize = 32;

/// RFC 3339 UTC as the schema accepts it (`YYYY-MM-DDTHH:MM:SS[.f]Z`, ≤24).
fn is_utc_stamp(s: &str) -> bool {
    let b = s.as_bytes();
    (20..=24).contains(&b.len()) && b.get(10) == Some(&b'T') && s.ends_with('Z')
}

pub fn collectors_body(dto: &CollectorStatusDto, paused: bool) -> Value {
    let rows: Vec<Value> = dto
        .collectors
        .iter()
        .take(MAX_COLLECTORS)
        .map(|c| {
            let last_upload = dto
                .results
                .iter()
                .filter(|r| r.ok && r.collector_id == c.id && is_utc_stamp(&r.at))
                .map(|r| r.at.as_str())
                .max();
            json!({
                "id": c.id,
                "enabled": c.enabled,
                "paused": paused,
                // The runtime does not keep a per-collector scan time or
                // queue depth yet; report "unknown" honestly.
                "lastScanAt": null,
                "lastUploadAt": last_upload,
                "pending": 0,
            })
        })
        .collect();
    json!({ "collectors": rows })
}
