//! `GET /dev/v1/collectors` (`$defs/collectors`): the same data the window's
//! `get_collector_status` shows, reduced to the contract's fields.

use crate::collector_status::row_status;
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
            let report = dto.scan.get(&c.id).cloned().unwrap_or_default();
            let this_run = dto
                .results
                .iter()
                .filter(|r| r.ok && r.collector_id == c.id)
                .map(|r| r.at.as_str());
            // The persisted time survives restarts; this run's results cover
            // an upload whose time could not be saved.
            let last_upload = this_run
                .chain(report.last_upload_at.as_deref())
                .filter(|at| is_utc_stamp(at))
                .max();
            // A switched-off collector never scanned (whatever an earlier
            // scan, before the switch flipped, left behind).
            let (last_scan, pending) = if c.enabled {
                let at = report.last_scan_at.filter(|at| is_utc_stamp(at));
                (at, report.pending)
            } else {
                (None, 0)
            };
            let device = dto.device_status(paused);
            let status = row_status(c.enabled, device, dto.line_status.get(&c.id).copied());
            json!({
                "id": c.id,
                "enabled": c.enabled,
                "paused": paused,
                "lastScanAt": last_scan,
                "lastUploadAt": last_upload,
                "pending": pending,
                "status": status.as_str(),
            })
        })
        .collect();
    let mut body = json!({ "collectors": rows });
    // Device-level, so top-level; omitted when the token cannot be read.
    if let Some(linked) = dto.linked {
        body["linked"] = Value::Bool(linked);
    }
    body
}
