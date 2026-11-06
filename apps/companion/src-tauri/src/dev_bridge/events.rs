//! Structured activity events for `GET /dev/v1/events` (dev-bridge/v1
//! `events`): a bounded ring of rows that are a timestamp, a built-in
//! collector id, an enum and integers — nothing else. No field is a `String`
//! from a path, file, server message or user/save name: [`CollectorId`] can
//! only be built from a registered manifest's static id, and server codes go
//! through [`ErrorCode::from_server`], which returns the enum.

use super::logbuf::parse_utc;
use super::paging::{invalid, page, wrap_next};
use base64::Engine;
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// Events retained; older ones are dropped (their cursors become a 400).
pub const CAPACITY: usize = 500;
const COUNT_MAX: u32 = 1_000_000;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const CURSOR_PREFIX: &str = "events";

/// What happened (`kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Scan,
    UploadOk,
    UploadRejected,
    UploadHeld,
    UploadRetry,
    LinkRequired,
    CollectorOn,
    CollectorOff,
    Error,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::UploadOk => "upload-ok",
            Self::UploadRejected => "upload-rejected",
            Self::UploadHeld => "upload-held",
            Self::UploadRetry => "upload-retry",
            Self::LinkRequired => "link-required",
            Self::CollectorOn => "collector-on",
            Self::CollectorOff => "collector-off",
            Self::Error => "error",
        }
    }
}

/// `DEV_BRIDGE_EVENT_ERROR_CODES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidEnvelope,
    InvalidPayload,
    PayloadTooLarge,
    CapturedAtInFuture,
    BlobQuotaExceeded,
    InvalidBlob,
    HashMismatch,
    UnsupportedMedia,
    BlobRefMismatch,
    BlobRefused,
    UnknownContract,
    UnknownContractVersion,
    RetiredContractVersion,
    Other,
}

const CODES: [(ErrorCode, &str); 13] = [
    (ErrorCode::InvalidEnvelope, "invalid_envelope"),
    (ErrorCode::InvalidPayload, "invalid_payload"),
    (ErrorCode::PayloadTooLarge, "payload_too_large"),
    (ErrorCode::CapturedAtInFuture, "captured_at_in_future"),
    (ErrorCode::BlobQuotaExceeded, "blob_quota_exceeded"),
    (ErrorCode::InvalidBlob, "invalid_blob"),
    (ErrorCode::HashMismatch, "hash_mismatch"),
    (ErrorCode::UnsupportedMedia, "unsupported_media"),
    (ErrorCode::BlobRefMismatch, "blob_ref_mismatch"),
    (ErrorCode::BlobRefused, "blob_refused"),
    (ErrorCode::UnknownContract, "unknown_contract"),
    (
        ErrorCode::UnknownContractVersion,
        "unknown_contract_version",
    ),
    (
        ErrorCode::RetiredContractVersion,
        "retired_contract_version",
    ),
];

impl ErrorCode {
    /// The server's `code`: listed codes map to themselves (exact match),
    /// every other one (unknown, uppercase API codes) to [`Self::Other`].
    pub fn from_server(code: &str) -> Self {
        CODES
            .iter()
            .find(|(_, s)| *s == code)
            .map_or(Self::Other, |(c, _)| *c)
    }

    pub fn as_str(self) -> &'static str {
        CODES
            .iter()
            .find(|(c, _)| *c == self)
            .map_or("other", |(_, s)| s)
    }
}

/// A built-in collector id: only ever a registered manifest's static id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollectorId(&'static str);

impl CollectorId {
    /// `None` for anything that is not a registered collector.
    pub fn known(id: &str) -> Option<Self> {
        crate::collector_view::known(id).ok().map(|m| Self(m.id))
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// One row before it is stamped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub collector: CollectorId,
    pub kind: Kind,
    pub count: Option<u32>,
    pub http_status: Option<u16>,
    pub error_code: Option<ErrorCode>,
}

impl Event {
    pub fn new(collector: CollectorId, kind: Kind) -> Self {
        Self {
            collector,
            kind,
            count: None,
            http_status: None,
            error_code: None,
        }
    }

    /// `count`, clamped to the schema's 0–1,000,000.
    pub fn count(self, n: usize) -> Self {
        let n = u32::try_from(n).unwrap_or(COUNT_MAX).min(COUNT_MAX);
        Self {
            count: Some(n),
            ..self
        }
    }

    /// `httpStatus`; a value outside 100–599 is left out.
    pub fn status(self, s: u16) -> Self {
        Self {
            http_status: (100..=599).contains(&s).then_some(s),
            ..self
        }
    }

    pub fn code(self, c: ErrorCode) -> Self {
        Self {
            error_code: Some(c),
            ..self
        }
    }

    fn to_json(self, ts: &str) -> Value {
        let mut m = Map::new();
        m.insert("ts".into(), json!(ts));
        m.insert("collectorId".into(), json!(self.collector.as_str()));
        m.insert("kind".into(), json!(self.kind.as_str()));
        if let Some(n) = self.count {
            m.insert("count".into(), json!(n));
        }
        if let Some(s) = self.http_status {
            m.insert("httpStatus".into(), json!(s));
        }
        if let Some(c) = self.error_code {
            m.insert("errorCode".into(), json!(c.as_str()));
        }
        Value::Object(m)
    }
}

struct Row {
    seq: usize,
    ts: String,
    at: (i64, u32),
    ev: Event,
}

/// The ring: `(next seq, rows oldest first)`.
#[derive(Default)]
pub struct EventLog {
    inner: Mutex<(usize, VecDeque<Row>)>,
}

impl EventLog {
    /// Store `ev` stamped `ts` (RFC 3339 UTC; anything else is dropped).
    pub fn push_at(&self, ev: Event, ts: String) {
        let Some(at) = parse_utc(&ts).filter(|_| ts.len() <= 24) else {
            // A format change must not empty the route unnoticed; the
            // message is fixed and carries no payload (never the text).
            eprintln!("dev-bridge events: dropped a row (timestamp not RFC 3339 UTC)");
            return;
        };
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        insert(&mut g, Row { seq: 0, ts, at, ev });
    }

    /// Store `ev` stamped `unix_ms` (milliseconds — the finest the schema's
    /// 24-char `ts` holds), moved to 1 ms after the newest row when it is
    /// not later than it. Every row is then strictly later than the one
    /// before, so a client polling with exclusive `since=<last ts>` misses
    /// nothing, even for events recorded in the same millisecond.
    pub fn push_now(&self, ev: Event, unix_ms: i64) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let after = g.1.back().map_or(i64::MIN, |r| {
            r.at.0
                .saturating_mul(1000)
                .saturating_add(i64::from(r.at.1 / 1_000_000))
                .saturating_add(1)
        });
        let ms = unix_ms.max(after);
        let ts = stamp_ms(ms);
        let at = (
            ms.div_euclid(1000),
            u32::try_from(ms.rem_euclid(1000)).unwrap_or(0) * 1_000_000,
        );
        insert(&mut g, Row { seq: 0, ts, at, ev });
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|p| p.into_inner()).1.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// One `events` page: `?since=` (exclusive) and `?cursor=` (this route's
    /// own; a dropped or foreign one is a 400 at `cursor`).
    pub fn page(&self, since: Option<&String>, cursor: Option<&String>) -> Result<Value, Value> {
        let since = match since {
            None => None,
            Some(s) => Some(
                parse_utc(s)
                    .filter(|_| s.len() <= 24)
                    .ok_or_else(|| invalid("since", "since is an RFC 3339 UTC time"))?,
            ),
        };
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let from = match cursor {
            None => 0,
            Some(c) => decode(c)
                .filter(|s| *s < g.0 && g.1.front().is_some_and(|r| r.seq <= *s))
                .ok_or_else(|| invalid("cursor", "Unknown or expired cursor"))?,
        };
        let selected: Vec<(usize, Value)> =
            g.1.iter()
                .filter(|r| r.seq >= from && since.is_none_or(|s| r.at > s))
                .map(|r| (r.seq, r.ev.to_json(&r.ts)))
                .collect();
        drop(g);
        let rows: Vec<Value> = selected.iter().map(|(_, v)| v.clone()).collect();
        let mut body = page(
            &rows,
            0,
            usize::MAX,
            |items, next| json!({ "events": items, "next": next }),
        );
        // `page` cursors index this selection; re-key them by seq.
        if let Some(idx) = body["next"].as_str().and_then(page_index) {
            let seq = selected.get(idx).map_or(from, |(s, _)| *s);
            body["next"] = wrap_next(Some(encode(seq)));
        }
        Ok(body)
    }
}

/// Append `row` with the next seq, evicting the oldest at capacity.
fn insert(g: &mut (usize, VecDeque<Row>), mut row: Row) {
    row.seq = g.0;
    g.0 += 1;
    if g.1.len() == CAPACITY {
        g.1.pop_front();
    }
    g.1.push_back(row);
}

/// `unix_ms` as `YYYY-MM-DDTHH:MM:SS.mmmZ` (24 chars).
pub fn stamp_ms(unix_ms: i64) -> String {
    let secs = rl_collectors::rfc3339_utc_from_unix_seconds(unix_ms.div_euclid(1000));
    let base = secs.strip_suffix('Z').unwrap_or(&secs);
    format!("{base}.{:03}Z", unix_ms.rem_euclid(1000))
}

fn page_index(c: &str) -> Option<usize> {
    super::paging::decode_cursor(Some(&c.to_owned())).ok()
}

pub fn encode(seq: usize) -> String {
    B64.encode(format!("{CURSOR_PREFIX}{seq}"))
}

fn decode(c: &str) -> Option<usize> {
    let s = String::from_utf8(B64.decode(c).ok()?).ok()?;
    s.strip_prefix(CURSOR_PREFIX)?.parse().ok()
}

/// The process-wide ring.
pub fn global() -> &'static EventLog {
    static B: OnceLock<EventLog> = OnceLock::new();
    B.get_or_init(EventLog::default)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// Record `kind` for collector `id` now (a non-collector id records nothing).
pub fn record(id: &str, kind: Kind, f: impl FnOnce(Event) -> Event) {
    if let Some(c) = CollectorId::known(id) {
        global().push_now(f(Event::new(c, kind)), now_ms());
    }
}

/// The event for one upload outcome.
pub fn for_outcome(c: CollectorId, o: &crate::upload::UploadOutcome, held: usize) -> Event {
    use crate::upload::UploadOutcome as O;
    match o {
        O::Accepted => Event::new(c, Kind::UploadOk).count(1),
        O::Rejected { status, code, .. } => {
            let ev = Event::new(c, Kind::UploadRejected).status(*status);
            if code.is_empty() {
                ev
            } else {
                ev.code(ErrorCode::from_server(code))
            }
        }
        O::Unauthorized => Event::new(c, Kind::LinkRequired).status(401),
        O::RateLimited { .. } => Event::new(c, Kind::UploadHeld).status(429).count(held),
        O::Deferred { status, code, .. } => {
            let code = code
                .as_deref()
                .map_or(ErrorCode::Other, ErrorCode::from_server);
            let ev = Event::new(c, Kind::UploadRetry).code(code);
            status.map_or(ev, |s| ev.status(s))
        }
        O::GaveUp { status, .. } => {
            let ev = Event::new(c, Kind::UploadRetry);
            status.map_or(ev, |s| ev.status(s))
        }
    }
}

/// Record one upload outcome for collector `id`. `held` (how many exports
/// a 429 holds) is only called for a 429 — counting it walks the queue.
pub fn record_outcome(id: &str, o: &crate::upload::UploadOutcome, held: impl FnOnce() -> usize) {
    if let Some(c) = CollectorId::known(id) {
        let held = match o {
            crate::upload::UploadOutcome::RateLimited { .. } => held(),
            _ => 0,
        };
        global().push_now(for_outcome(c, o, held), now_ms());
    }
}

/// The user switched collector `id` on or off.
pub fn record_toggle(id: &str, on: bool) {
    let kind = if on {
        Kind::CollectorOn
    } else {
        Kind::CollectorOff
    };
    record(id, kind, |e| e);
}
